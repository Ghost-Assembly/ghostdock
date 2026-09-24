// A shell inside a container, end to end in a real browser.
//
// Needs a live server with a FRESH database and a Docker daemon with at
// least one running stack registered.
//
// Plain ESM rather than TypeScript, deliberately: these are short linear
// scripts run directly by `node` with no build step, and the only types
// involved belong to Playwright.
import { chromium, devices } from 'playwright';

const B = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
page.on('console', m => { if (m.type() === 'error' && !m.text().includes('Failed to load resource')) problems.push(m.text()); });

await page.goto(B, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.entry-heading');
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.waitForSelector('.verdict-line, .state-note');

// Reach the shell the way a person would: board → stack → container. By
// name rather than position: the board orders by severity, so whatever else
// is on the host decides which row comes first.
await page.click('a.row-link:has-text("shellbox")');
await page.waitForSelector('.actions');
await page.waitForSelector('a.row-aside:has-text("Shell")', { timeout: 20000 });
// Read the container row only once it has arrived; the list is fetched.
console.log('container row :', (await page.textContent('a.row-link:has(+ .row-aside) .row-name')).trim());
await page.click('a.row-aside:has-text("Shell")');
await page.waitForSelector('h1.wordmark:has-text("Shell")');

// The socket must open before the Run button is usable.
await page.waitForFunction(() => !document.querySelector('button[type=submit]').disabled, null, { timeout: 20000 });
console.log('status        :', (await page.textContent('.entry-note')).trim());

await page.fill('input[placeholder="ls -la"]', 'echo hello from the container; id -u');
await page.click('button[type=submit]');
await page.waitForFunction(() => document.querySelector('pre.log')?.textContent.includes('hello from the container'), null, { timeout: 20000 });

// stderr must stay distinguishable in here too.
await page.fill('input[placeholder="ls -la"]', 'ls /definitely-not-here');
await page.click('button[type=submit]');
await page.waitForFunction(() => document.querySelectorAll('pre.log .log-stderr').length > 0, null, { timeout: 20000 });

const text = await page.textContent('pre.log');
console.log('scrollback    :', JSON.stringify(text.split('\n').filter(Boolean).slice(0, 5)));
const stderrLines = await page.$$eval('.log-stderr', els => els.map(e => e.textContent.trim()));
console.log('stderr lines  :', JSON.stringify(stderrLines));

// A shell is one long request. Changing the password must close it rather
// than let it outlive the session that opened it.
const changed = await page.evaluate(async () => (await fetch('/api/v1/auth/password', {
  method: 'PUT',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ current: 'correct horse battery staple', new: 'a different long passphrase' }),
})).status);
if (changed !== 204) problems.push(`could not change the password: ${changed}`);
await page.waitForFunction(() => document.querySelector('pre.log')?.textContent.includes('access was revoked'), null, { timeout: 10000 });
await page.waitForFunction(() => document.querySelector('button[type=submit]').disabled, null, { timeout: 10000 });
console.log('after revoke  :', (await page.$$eval('pre.log .log-stderr', els => els.at(-1).textContent)).trim());

const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`horizontal overflow ${overflow}px`);
await page.screenshot({ path: (process.env.SHOTS ?? 'target/e2e-shots') + '/shell.png' });
await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nno console errors, no overflow');
process.exit(problems.length ? 1 : 0);
