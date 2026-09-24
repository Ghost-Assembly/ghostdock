// The Host screen: figures now, charts that fill in, disks, the containers
// using the most, and a live value that changes without a reload.
//
// Needs a live server with the Chatter fixture deployed. See run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const problems = [];
const browser = await chromium.launch();

for (const [label, options] of [['phone', devices['iPhone 14']], ['desktop', { viewport: { width: 1440, height: 900 } }]]) {
  const ctx = await browser.newContext({ ...options, colorScheme: 'dark' });
  const page = await ctx.newPage();
  page.on('pageerror', e => problems.push(`${label}: ${e.message}`));
  await page.goto(BASE, { waitUntil: 'networkidle' });
  await page.fill('input[type=text]', 'admin');
  await page.fill('input[type=password]', 'correct horse battery staple');
  await page.click('button[type=submit]');
  await page.click('.nav-item:has-text("Host")');
  await page.waitForSelector('.chart-area', { timeout: 30000 });

  // Wait for a CPU figure, then for it to change: the socket delivers ticks.
  const value = () => page.textContent('.chart:has(.chart-title:text("CPU")) .chart-value');
  await page.waitForFunction(() => /cores/.test(document.querySelector('.chart .chart-value')?.textContent ?? ''), null, { timeout: 30000 });
  const first = await value();
  await page.waitForFunction(v => document.querySelector('.chart .chart-value')?.textContent !== v, first, { timeout: 20000 })
    .catch(() => problems.push(`${label}: the CPU figure never changed`));
  console.log(`${label.padEnd(8)}:`, `CPU ${first} then ${await value()}`);

  const disks = await page.$$eval('.usage', els => els.length);
  const top = await page.$$eval('.rows-top .row-name', els => els.map(e => e.textContent));
  console.log(`${label.padEnd(8)}:`, `${disks} disk bar(s); top: ${top.slice(0, 3).join(', ')}`);
  if (!disks) problems.push(`${label}: no disk bar`);
  if (!top.some(n => n.startsWith('chatter'))) problems.push(`${label}: the busy container is not among the top`);

  await page.waitForSelector('h2.group-heading:text("Sizing")')
    .catch(() => problems.push(`${label}: no Sizing section`));
  await page.click('.range-picker button:has-text("24h")');
  await page.waitForTimeout(1000);
  const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  if (sideways > 0) problems.push(`${label}: scrolls sideways by ${sideways}px`);
  await page.screenshot({ path: `${SHOTS}/host-${label}.png`, fullPage: true });
  await ctx.close();
}

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nThe Host screen works at both sizes.');
process.exit(problems.length ? 1 : 0);
