// Finding the stacks in a repository and registering several at once.
//
// Needs a fresh server with one repository registered. See run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.click('.nav-item:has-text("Settings")');
await page.click('a:has-text("Repositories and credentials")');
await page.screenshot({ path: `${SHOTS}/sources.png`, fullPage: true });
await page.click('a.row-link:has-text("Find stacks")');

await page.click('button:has-text("Look")');
await page.waitForSelector('.check', { timeout: 30000 });
const offered = await page.$$eval('.check', els => els.map(e => ({
  name: e.querySelector('.check-name').textContent,
  checked: e.querySelector('input').checked,
})));
console.log('found         :', offered.map(o => `${o.name}${o.checked ? ' [x]' : ''}`).join(', '));
if (offered.length !== 3 || !offered.every(o => o.checked)) problems.push('all three should be found and chosen');
console.log('pattern       :', await page.inputValue('input[placeholder*="compose.yaml"]'));
await page.screenshot({ path: `${SHOTS}/discover.png`, fullPage: true });

// Leave one out.
await page.click('.check:has(.check-name:text-is("beta")) input');
await page.click('button:has-text("Register 2 stacks")');
await page.waitForSelector('h2.group-heading:has-text("Registered 2")', { timeout: 20000 });
const registered = await page.$$eval('.rows .row-name', els => els.map(e => e.textContent));
console.log('registered    :', registered.join(', '));

// Looking again shows what is already registered, and still offers beta.
await page.click('button:has-text("Look")');
await page.waitForSelector('.check');
const again = await page.$$eval('.check', els => els.map(e => [
  e.querySelector('.check-name').textContent, e.querySelector('input').disabled,
]));
console.log('looked again  :', again.map(([n, d]) => `${n}${d ? ' (registered)' : ''}`).join(', '));
const beta = again.find(([n]) => n === 'beta');
if (!beta || beta[1]) problems.push('beta should still be offered');
if (again.filter(([, d]) => d).length !== 2) problems.push('the two registered should not be offered again');

// The credential can be changed in place, without re-registering stacks.
await page.selectOption('select', '');
await page.click('button:has-text("Save credential")');
await page.waitForSelector('text=Saved. No credential will be sent.', { timeout: 10000 });
console.log('credential    : saved in place');

const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`page scrolls horizontally by ${overflow}px`);
await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow.');
process.exit(problems.length ? 1 : 0);
