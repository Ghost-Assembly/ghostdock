// A server that stops answering must not stop the app. The board's request
// is held open and never answered here; the board must give up at its
// deadline and say so, and the rest of the app must stay usable meanwhile.
//
// Needs a fresh server. See run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.waitForSelector('.nav-item', { timeout: 30000 });

// From now on, the board's request goes out and nothing comes back.
await page.route('**/api/v1/hosts/1/stacks', () => {});
await page.click('.nav-item:has-text("Settings")');
await page.click('.nav-item:has-text("Stacks")');
const t0 = Date.now();

// While it waits, everything else still works.
await page.click('.nav-item:has-text("Settings")');
const usable = await page.waitForSelector('h1.wordmark:has-text("Settings")', { timeout: 2000 }).then(() => true, () => false);
console.log('while stalled :', usable ? 'navigation still works' : 'STUCK');
if (!usable) problems.push('the app stopped responding while a request hung');
await page.click('.nav-item:has-text("Stacks")');

const gaveUp = await page.waitForSelector('text=did not answer in time', { timeout: 25000 }).then(() => true, () => false);
console.log('board         :', gaveUp ? `gave up after ${Math.round((Date.now() - t0) / 1000)}s and said so` : 'STILL LOADING');
if (!gaveUp) problems.push('a request that never returns left the board loading');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo request can hang the app.');
process.exit(problems.length ? 1 : 0);
