// The main thread must stay free. Measures long tasks (the browser's own
// "this blocked input" signal) on a phone-like CPU while the app does its
// heaviest things: a full board under bursts of live events, searching a
// long log one keystroke at a time, and following a chatty container.
//
// Needs a live server with the Chatter fixture and many stacks. See run.sh.
import { execFileSync } from 'node:child_process';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
// A single task longer than this is a visible hitch in a tap or a scroll.
// Measured on a CPU slowed four times, so this is generous for a phone.
const BUDGET_MS = Number(process.env.JANK_BUDGET_MS ?? 100);
// INP's own line between good and needs-improvement.
const INPUT_BUDGET_MS = 200;
const FRAME_BUDGET_MS = 200;

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['Pixel 7'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
const cdp = await ctx.newCDPSession(page);
await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });

// Three views of the same question. Long tasks: one job blocking input.
// Input latency (Event Timing, what INP is built on): how long a key or tap
// took to show. Frame gaps: how long the screen went without repainting,
// which catches many short jobs starving input as well as one long one.
await page.addInitScript(() => {
  window.__long = [];
  window.__input = [];
  window.__gaps = [];
  new PerformanceObserver(list => {
    for (const e of list.getEntries()) window.__long.push(e.duration);
  }).observe({ type: 'longtask', buffered: true });
  new PerformanceObserver(list => {
    for (const e of list.getEntries()) window.__input.push(e.duration);
  }).observe({ type: 'event', durationThreshold: 16, buffered: true });
  let last = performance.now();
  const frame = now => { window.__gaps.push(now - last); last = now; requestAnimationFrame(frame); };
  requestAnimationFrame(frame);
});

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.waitForSelector('.row-link', { timeout: 60000 });

async function phase(name, work) {
  await page.evaluate(() => { window.__long = []; window.__input = []; window.__gaps = []; });
  const failed = await work().then(() => null, e => e.message.split('\n')[0]);
  const m = await page.evaluate(() => ({ long: window.__long, input: window.__input, gaps: window.__gaps }));
  const max = xs => Math.round(Math.max(0, ...xs));
  console.log(`${name.padEnd(22)}: longest task ${max(m.long)}ms, slowest input ${max(m.input)}ms, ` +
    `longest frame gap ${max(m.gaps)}ms${failed ? `, FAILED: ${failed}` : ''}`);
  if (name.startsWith('calibration')) return m;
  if (failed) problems.push(`${name}: ${failed}`);
  if (max(m.long) > BUDGET_MS) problems.push(`${name}: a ${max(m.long)}ms task (budget ${BUDGET_MS}ms)`);
  if (max(m.input) > INPUT_BUDGET_MS) problems.push(`${name}: input took ${max(m.input)}ms to show (budget ${INPUT_BUDGET_MS}ms)`);
  if (max(m.gaps) > FRAME_BUDGET_MS) problems.push(`${name}: ${max(m.gaps)}ms without a frame (budget ${FRAME_BUDGET_MS}ms)`);
  return m;
}

// Proves the instrument works: a deliberate 200 ms block must register,
// or every zero below would mean nothing.
await phase('calibration (200ms)', async () => {
  await page.evaluate(() => new Promise(r => setTimeout(() => {
    const t = performance.now(); while (performance.now() - t < 200) {} r();
  }, 0)));
  await page.waitForTimeout(200);
});
const calibrated = await page.evaluate(() => window.__long.length && Math.max(...window.__gaps) >= 150);
if (!calibrated) problems.push('the long-task instrument recorded nothing for a deliberate block');

await phase('board, event bursts', async () => {
  for (let i = 0; i < 6; i++) {
    const args = i % 2 ? ['start', 'chatter-talk-1'] : ['stop', '-t', '0', 'chatter-talk-1'];
    execFileSync('docker', args, { stdio: 'ignore' });
    await page.waitForTimeout(700);
  }
});

await page.click('.row-link:has(.row-name:text("chatter"))');
await page.click('a.row-link:has(.row-count:text("Logs"))', { timeout: 20000 });
await page.waitForSelector('pre.log .log-line', { timeout: 20000 });

await phase('log search, typing', async () => {
  await page.locator('input[type=search]').pressSequentially('lorem ipsum 4', { delay: 60, timeout: 20000 });
  await page.fill('input[type=search]', '');
  await page.waitForTimeout(300);
});

await phase('following, 200 lines/s', async () => {
  await page.click('button:has-text("Follow")');
  await page.waitForTimeout(6000);
});
await page.waitForTimeout(14000);
const full = await page.$$eval('pre.log .log-line', els => els.length);
console.log('lines on screen       :', full);

// Taking a screenshot needs a frame; a page too busy to render fails here.
await page.screenshot({ path: `${process.env.SHOTS ?? 'target/e2e-shots'}/jank-full.png` });

// The worst case for search: every line on screen, re-filtered per key.
await phase('search, full log', async () => {
  await page.locator('input[type=search]').pressSequentially('consectetur 12', { delay: 60, timeout: 20000 });
  await page.fill('input[type=search]', '');
  await page.waitForTimeout(300);
});

// Live charts: a tick every 5 s redraws them, and a range change swaps
// every series at once.
await page.goto(`${BASE}/host`);
await page.waitForSelector('.chart-area', { timeout: 30000 });
await phase('host screen, live', async () => {
  await page.waitForTimeout(12000);
  await page.click('.range-picker button:has-text("7d")');
  await page.waitForTimeout(1500);
  await page.click('.range-picker button:has-text("1h")');
  await page.waitForTimeout(3000);
});

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nWithin budget.');
process.exit(problems.length ? 1 : 0);
