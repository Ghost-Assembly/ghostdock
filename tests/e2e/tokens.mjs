// API tokens, end to end: create one in the browser with a single
// permission, use it from outside the browser, find it limited to exactly
// that, then revoke it and find it dead.
//
// Needs a live server with a FRESH database. See tests/e2e/run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const PASSWORD = 'correct horse battery staple';

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.entry-heading');
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', PASSWORD);
await page.click('button[type=submit]');
await page.waitForSelector('.nav-item', { timeout: 30000 });

await page.click('.nav-item:has-text("Settings")');
await page.click('a:has-text("API tokens")');
await page.waitForSelector('h1.wordmark:has-text("API tokens")');

// 1. Nothing is ticked by default: a token grants nothing until chosen.
const preticked = await page.$$eval('.check input', els => els.filter(e => e.checked).length);
if (preticked) problems.push(`${preticked} permissions were ticked before anyone chose them`);
const expiry = await page.$eval('select', s => s.value);
console.log('defaults      :', `${preticked} ticked, expires in ${expiry} days`);

await page.fill('input[type=text]', 'assistant');
await page.click('.check:has(.check-name:text("host.view"))');
await page.click('button:has-text("Create token")');
await page.waitForSelector('pre.secret');
const secret = (await page.textContent('pre.secret')).trim();
console.log('revealed      :', `${secret.slice(0, 11)}… (${secret.length} chars)`);
await page.screenshot({ path: `${SHOTS}/token-created.png`, fullPage: true });
const command = (await page.$$eval('pre.secret', els => els.map(e => e.textContent)))[1] ?? '';
const expected = `claude mcp add --transport http ghostdock ${BASE}/mcp --header "Authorization: Bearer ${secret}"`;
console.log('claude code   :', command === expected ? 'setup command shown' : `WRONG: ${command.slice(0, 80)}`);
if (command !== expected) problems.push('the Claude Code command is not the one to paste');
const secretOverflow = await page.$eval('pre.secret', el => el.scrollWidth - el.clientWidth);
if (secretOverflow > 0) problems.push(`the secret runs out of its box by ${secretOverflow}px`);

// 2. Outside the browser, with no cookie: read works, anything else does not.
const call = (method, path, body) => fetch(`${BASE}/api/v1${path}`, {
  method,
  headers: { authorization: `Bearer ${secret}`, 'content-type': 'application/json' },
  body: body ? JSON.stringify(body) : undefined,
});
const read = await call('GET', '/hosts');
const write = await call('POST', '/hosts/1/stacks', { name: 'Nope', compose_yaml: 'services: {}\n' });
const mint = await call('POST', '/tokens', { name: 'more', permissions: ["shell.open"], expires_in_days: null });
console.log('with token    :', `read ${read.status}, register ${write.status}, mint ${mint.status}`);
if (read.status !== 200) problems.push(`a read token could not read: ${read.status}`);
if (write.status !== 403) problems.push(`a read token could register a stack: ${write.status}`);
if (mint.status !== 403) problems.push(`a token could mint another: ${mint.status}`);

// 3. The row reports it; revoking takes two taps and ends it at once.
await page.click('button:has-text("I have copied it")');
await page.reload({ waitUntil: 'networkidle' });
const detail = await page.textContent('.rows .row-detail');
console.log('listed        :', detail);
if (!detail.includes('used')) problems.push('the row does not say when the token was used');
await page.click('.row-action:has-text("Revoke")');
await page.waitForTimeout(300);
if ((await call('GET', '/hosts')).status !== 200) problems.push('one tap revoked the token');
await page.click('.row-action:has-text("Revoke it")');
await page.waitForSelector('text=No tokens yet.');
const after = await call('GET', '/hosts');
console.log('after revoke  :', after.status);
if (after.status !== 401) problems.push(`a revoked token still works: ${after.status}`);

const overflow = await page.evaluate(() =>
  document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`page scrolls horizontally by ${overflow}px`);

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow.');
process.exit(problems.length ? 1 : 0);
