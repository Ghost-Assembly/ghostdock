// Logs across containers: a stack's containers followed together, each
// line labeled with its container, searched, narrowed to stderr, paused and
// resumed, downloaded, and the socket closed on leaving. Then the same
// screen under load on a throttled CPU, held to jank's budgets.
//
// Needs a live server with the Multilogs fixture deployed as stack 1. See
// run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const PROJECT = 'ghostdock-e2e-multilogs';
const NORTH = `${PROJECT}-north-1`;
const SOUTH = `${PROJECT}-south-1`;
// The same budgets as jank.mjs, on the same fourfold CPU slowdown.
const BUDGET_MS = Number(process.env.JANK_BUDGET_MS ?? 100);
const INPUT_BUDGET_MS = 200;
const FRAME_BUDGET_MS = 200;

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
const sockets = [];
page.on('websocket', ws => { if (ws.url().includes('/logs/socket')) sockets.push(ws); });

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');

// In from the stack's own page.
await page.click(`.row-link:has(.row-name:text("${PROJECT}"))`, { timeout: 30000 });
await page.click('a.button:has-text("Follow its logs")', { timeout: 20000 });
await page.waitForURL(/\/logs\?stack=\d+$/);
// Named once the stacks are read.
await page.waitForSelector(`.mlog-summary:text-is("Stack ${PROJECT}")`, { timeout: 10000 })
  .catch(async () => problems.push(`the picker says "${(await page.textContent('.mlog-summary')).trim()}"`));
const current = await page.getAttribute('.nav-item:has-text("Logs")', 'aria-current');
if (current !== 'page') problems.push('Logs is not the current section');

// Both containers, each line labeled with its name in a column: without
// the stack's name in front, which would make every label look alike in a
// narrow column, and in full on hover.
const labeled = (name, short) => page.waitForSelector(
  `.mlog-line .mlog-name[title="${name}"]:text-is("${short}")`, { timeout: 20000 },
).catch(() => problems.push(`no line labeled ${short} for ${name}`));
await labeled(NORTH, 'north-1');
await labeled(SOUTH, 'south-1');
const colors = await page.$$eval('.mlog-line .mlog-name', els =>
  [...new Set(els.map(e => getComputedStyle(e).color))]);
if (colors.length !== 1) problems.push(`container labels differ in color: ${colors}`);
console.log("labels        :", "north-1, south-1; one label color");

const lines = () => page.$$eval('.mlog-line', els => els.map(e => ({
  name: e.querySelector('.mlog-name')?.title ?? '',
  text: e.querySelector('.mlog-text')?.textContent ?? '',
  stderr: e.classList.contains('log-stderr'),
})));

// A search is case-insensitive, narrows the lines and marks what matched,
// without color.
await page.fill('input[type=search]', 'NORTH WARN');
await page.waitForTimeout(500);
const found = await lines();
if (!found.length || !found.every(l => l.text.toLowerCase().includes('north warn') && l.name === NORTH)) {
  problems.push(`search shows ${JSON.stringify(found.slice(0, 3))}`);
}
const marks = await page.$$eval('mark.mlog-match', els => els.map(e => ({
  text: e.textContent,
  underlined: getComputedStyle(e).textDecorationLine.includes('underline'),
  background: getComputedStyle(e).backgroundColor,
})));
if (!marks.length || !marks.every(m => m.text.toLowerCase() === 'north warn')) problems.push(`marks: ${JSON.stringify(marks.slice(0, 3))}`);
if (!marks.every(m => m.underlined && m.background === 'rgba(0, 0, 0, 0)')) problems.push('matches are not marked by underline alone');
console.log('search        :', `${found.length} lines, ${marks.length} marked`);
await page.fill('input[type=search]', '');

// Only stderr.
await page.click('button:has-text("Only stderr")');
await page.waitForTimeout(300);
const errs = await lines();
if (!errs.length || !errs.every(l => l.stderr && l.text.includes('warn'))) problems.push('only stderr shows stdout');
await page.click('button:has-text("Only stderr")');
console.log('only stderr   :', `${errs.length} lines`);

// Pause holds new lines and says how many; resume shows them.
const last = async () => (await lines()).at(-1)?.text;
await page.click('button:has-text("Pause")');
const before = await last();
await page.waitForTimeout(2500);
const during = await last();
if (during !== before) problems.push('lines kept arriving while paused');
const held = (await page.textContent('p[role=status]:has-text("Paused")').catch(() => '')).trim();
if (!/Paused\. \d+ new lines? (is|are) waiting\./.test(held)) problems.push(`while paused it says "${held}"`);
await page.click('button:has-text("Resume")');
await page.waitForFunction(b => {
  const all = [...document.querySelectorAll('.mlog-line .mlog-text')];
  return all.length && all.at(-1).textContent !== b;
}, before, { timeout: 10000 }).catch(() => problems.push('resuming showed nothing new'));
if (await page.$('p[role=status]:has-text("Paused")')) problems.push('still says paused after resuming');
console.log('pause         :', held);
// Reading the latest, the view keeps up as lines arrive.
await page.waitForTimeout(1500);
const behind = await page.$eval('pre.log', el => el.scrollHeight - el.scrollTop - el.clientHeight);
if (behind > 60) problems.push(`the view did not keep up with new lines (${behind}px from the bottom)`);
await page.screenshot({ path: `${SHOTS}/multilogs.png` });

// The download holds both containers' lines.
const href = await page.getAttribute('a:has-text("Download")', 'href');
if (!/\/logs\.txt\?stack=\d+$/.test(href ?? '')) problems.push(`download link ${href}`);
const file = await (await page.request.get(`${BASE}${href}`)).text();
if (!file.includes(`${NORTH} north`) || !file.includes(`${SOUTH} south`)) problems.push('the download lacks a container');
console.log('download      :', `${file.split('\n').length - 1} lines`);

// Leaving the screen closes its socket.
if (!sockets.length) problems.push('no logs socket was opened');
await page.click('.nav-item:has-text("Stacks")');
await page.waitForTimeout(1500);
const open = sockets.filter(ws => !ws.isClosed()).length;
if (open) problems.push(`${open} logs socket(s) still open after leaving`);
console.log('on leaving    :', `${sockets.length} opened, ${open} still open`);

// Under load on a slow phone: following about 200 lines a second while
// typing a search, with jank's instruments and budgets.
const cdp = await ctx.newCDPSession(page);
await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
await page.click(`.row-link:has(.row-name:text("${PROJECT}"))`, { timeout: 30000 });
await page.click('a.button:has-text("Follow its logs")', { timeout: 20000 });
await page.waitForSelector('.mlog-line', { timeout: 20000 });
await page.evaluate(() => {
  window.__long = []; window.__input = []; window.__gaps = [];
  new PerformanceObserver(list => { for (const e of list.getEntries()) window.__long.push(e.duration); })
    .observe({ type: 'longtask' });
  new PerformanceObserver(list => { for (const e of list.getEntries()) window.__input.push(e.duration); })
    .observe({ type: 'event', durationThreshold: 16 });
  let lastFrame = performance.now();
  const frame = now => { window.__gaps.push(now - lastFrame); lastFrame = now; requestAnimationFrame(frame); };
  requestAnimationFrame(frame);
});
await page.waitForTimeout(500);
async function phase(name, work) {
  await page.evaluate(() => { window.__long = []; window.__input = []; window.__gaps = []; });
  const failed = await work().then(() => null, e => e.message.split('\n')[0]);
  const m = await page.evaluate(() => ({ long: window.__long, input: window.__input, gaps: window.__gaps }));
  const max = xs => Math.round(Math.max(0, ...xs));
  console.log(`${name.padEnd(22)}: longest task ${max(m.long)}ms, slowest input ${max(m.input)}ms, ` +
    `longest frame gap ${max(m.gaps)}ms${failed ? `, FAILED: ${failed}` : ''}`);
  if (failed) problems.push(`${name}: ${failed}`);
  if (max(m.long) > BUDGET_MS) problems.push(`${name}: a ${max(m.long)}ms task (budget ${BUDGET_MS}ms)`);
  if (max(m.input) > INPUT_BUDGET_MS) problems.push(`${name}: input took ${max(m.input)}ms to show (budget ${INPUT_BUDGET_MS}ms)`);
  if (max(m.gaps) > FRAME_BUDGET_MS) problems.push(`${name}: ${max(m.gaps)}ms without a frame (budget ${FRAME_BUDGET_MS}ms)`);
}
await phase('logs, following', async () => {
  await page.waitForTimeout(6000);
});
await phase('logs, search typing', async () => {
  await page.locator('input[type=search]').pressSequentially('south line 1', { delay: 60, timeout: 20000 });
  await page.waitForTimeout(1500);
  await page.fill('input[type=search]', '');
  await page.waitForTimeout(500);
});
await phase('logs, pause and resume', async () => {
  await page.click('button:has-text("Pause")');
  await page.waitForTimeout(4000);
  await page.click('button:has-text("Resume")');
  await page.waitForTimeout(3000);
});
const onScreen = await page.$$eval('.mlog-line', els => els.length);
console.log('lines on screen       :', onScreen);
await page.screenshot({ path: `${SHOTS}/multilogs-load.png` });

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo problems.');
process.exit(problems.length ? 1 : 0);
