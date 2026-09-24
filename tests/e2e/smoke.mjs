// Mobile smoke path for the GhostDock web client.
//
// Runs against a live server with a *fresh* database, because it exercises
// first-run setup. See `just test-web`.
//
// Kept as a script rather than a test-runner project: it is one linear
// journey, and the assertions worth making are about what a person sees.
//
// Plain ESM rather than TypeScript, deliberately: these are three short linear
// scripts run directly by `node` with no build step, and the only types
// involved belong to Playwright. A compile step here would add a toolchain to
// the repository for no checking it does not already get.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const OUT = process.env.SHOTS ?? 'target/e2e-shots';
const PASSWORD = 'correct horse battery staple';

const browser = await chromium.launch();
const context = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await context.newPage();

const problems = [];
// A deliberate 4xx below makes the browser log a failed-resource error. That
// is the server rejecting bad input correctly, so only unexpected console
// errors are collected.
let expectRequestFailure = false;
page.on('console', m => {
  if (m.type() !== 'error') return;
  if (expectRequestFailure && m.text().includes('Failed to load resource')) return;
  problems.push(`console: ${m.text()}`);
});
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

async function noHorizontalScroll(label) {
  const overflow = await page.evaluate(() =>
    document.documentElement.scrollWidth - document.documentElement.clientWidth);
  if (overflow > 0) problems.push(`${label}: page scrolls horizontally by ${overflow}px`);
}

// 1. First run
await page.goto(BASE, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.entry-heading', { timeout: 15000 });
console.log('setup heading :', await page.textContent('h1.entry-heading'));
await noHorizontalScroll('setup');
await page.screenshot({ path: `${OUT}/1-setup.png` });

// 2. Reject a short password, showing the server's own message
expectRequestFailure = true;
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'short');
await page.click('button[type=submit]');
await page.waitForSelector('.notice', { timeout: 15000 });
console.log('short pw error:', (await page.textContent('.notice')).trim());
await page.screenshot({ path: `${OUT}/2-password-rejected.png` });

expectRequestFailure = false;

// 3. Create the account
await page.fill('input[type=password]', PASSWORD);
await page.click('button[type=submit]');
// The board shows a verdict when stacks exist and an empty state when they
// do not. Both are correct; the test must not require containers to be
// running on whatever machine it is pointed at.
await page.waitForSelector('.verdict-line, .state-note', { timeout: 15000 });
const hasStacks = await page.$('.verdict-line');
if (hasStacks) {
  console.log('verdict       :', await page.textContent('.verdict-line'));
  console.log('counts        :', await page.textContent('.verdict-count'));
  const headings = await page.$$eval('.group-heading', els => els.map(e => e.textContent));
  console.log('groups        :', JSON.stringify(headings));
  const rows = await page.$$eval('.row-link', els => els.map(e => {
    const bar = e.querySelector('.row-bar');
    return `${e.querySelector('.row-name').textContent} ${bar.dataset.state}`;
  }));
  console.log('rows          :', JSON.stringify(rows, null, 0));
} else {
  console.log('board         : empty state (no stacks on this host)');
  if (!(await page.$('a[href="/stacks/new"]'))) {
    problems.push('empty board offers no way to add a stack');
  }
}
await noHorizontalScroll('board');
await page.screenshot({ path: `${OUT}/3-board.png` });

// 4. Touch targets must be reachable with a thumb
const small = await page.$$eval('.row-link, .nav-item, .button', els =>
  els.filter(e => e.getBoundingClientRect().height < 44)
     .map(e => `${e.className} ${Math.round(e.getBoundingClientRect().height)}px`));
if (small.length) problems.push(`touch targets under 44px: ${small.join(', ')}`);

// 5. Settings, via the bottom nav
await page.click('.nav-item:has-text("Settings")');
await page.waitForSelector('h1.wordmark:has-text("Settings")', { timeout: 15000 });
console.log('settings url  :', new URL(page.url()).pathname);
await page.waitForSelector('.rows .row-name', { timeout: 15000 });
console.log('host row      :', (await page.textContent('.row-link:has(.row-bar) .row-detail')).trim());
await noHorizontalScroll('settings');
await page.screenshot({ path: `${OUT}/4-settings.png` });

// 6. A client route must survive a refresh
await page.goto(`${BASE}/settings`, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.wordmark:has-text("Settings")', { timeout: 15000 });
console.log('after refresh : settings still rendered');

// 7. Light mode renders too
const light = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'light' });
const lightPage = await light.newPage();
await lightPage.context().addCookies(await context.cookies());
await lightPage.goto(BASE, { waitUntil: 'networkidle' });
await lightPage.waitForSelector('.verdict-line, .state-note', { timeout: 15000 });
await lightPage.screenshot({ path: `${OUT}/5-board-light.png` });
console.log('light mode    : rendered');

// 8. Sign out
await page.goto(BASE, { waitUntil: 'networkidle' });
await page.click('.nav-item:has-text("Settings")');
await page.waitForSelector('button:has-text("Sign out")', { timeout: 15000 });
await page.click('button:has-text("Sign out")');
await page.waitForSelector('h1.entry-heading:has-text("Sign in")', { timeout: 15000 });
console.log('after signout :', await page.textContent('h1.entry-heading'));

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow, no small touch targets.');
process.exit(problems.length ? 1 : 0);
