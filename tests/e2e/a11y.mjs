// Accessibility: every main screen, in both themes, at both sizes, checked
// by axe against WCAG 2 A and AA (contrast, names, labels, roles), and a
// screenshot of each for a person to look at. Axe finds perhaps a third of
// what matters; the checks after it cover a few things it cannot see:
// a theme chosen on the device beating the device's own, a chart read from
// the keyboard, and a two-tap button that keeps focus and gives up by
// itself.
//
// Screenshots land in $SHOTS/a11y/<viewport>-<theme>-<screen>.png.
//
// Needs a live server with the Ticker fixture deployed. See run.sh.
import { mkdirSync } from 'node:fs';
import { AxeBuilder } from '@axe-core/playwright';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = `${process.env.SHOTS ?? 'target/e2e-shots'}/a11y`;
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];
mkdirSync(SHOTS, { recursive: true });

const problems = [];
const browser = await chromium.launch();

// Signed in once; each context below starts with the same cookie.
const first = await browser.newContext({ ...devices['iPhone 14'] });
const signIn = await first.newPage();
await signIn.goto(BASE, { waitUntil: 'networkidle' });
await signIn.fill('input[type=text]', 'admin');
await signIn.fill('input[type=password]', 'correct horse battery staple');
await signIn.click('button[type=submit]');
await signIn.waitForSelector('.row-link:has(.row-name:text("ticker"))', { timeout: 30000 });
const stack = await signIn.getAttribute('.row-link:has(.row-name:text("ticker"))', 'href');
await signIn.goto(`${BASE}${stack}`);
const logs = await signIn.getAttribute('a.row-link:has(.row-count:text("Logs"))', 'href', { timeout: 30000 });
const session = await first.storageState();
await first.close();

// Each screen, and what shows it has its content.
const SCREENS = [
  ['board', '/', '.verdict-line'],
  ['stack', stack, 'a.row-link:has(.row-count:text("Logs"))'],
  ['host', '/host', '.usage'],
  ['updates', '/updates', '.verdict-line'],
  ['settings', '/settings', '.rows .row-name'],
  ['tokens', '/tokens', 'form'],
  ['activity', '/activity', '.rows .row-name'],
  ['logs', logs, 'pre.log .log-line'],
  ['multilogs', '/logs', '.mlog-summary'],
];

const VIEWPORTS = [
  ['phone', { ...devices['iPhone 14'] }],
  ['desktop', { viewport: { width: 1280, height: 800 } }],
];

for (const [size, options] of VIEWPORTS) {
  for (const theme of ['light', 'dark']) {
    // The device asks for the other theme: the one chosen here must win.
    const device = theme === 'light' ? 'dark' : 'light';
    const ctx = await browser.newContext({ ...options, colorScheme: device, storageState: session });
    await ctx.addInitScript(t => localStorage.setItem('ghostdock.theme', t), theme);
    const page = await ctx.newPage();
    page.on('pageerror', e => problems.push(`${size} ${theme}: ${e.message}`));

    for (const [name, path, ready] of SCREENS) {
      const label = `${size} ${theme} ${name}`;
      await page.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
      await page.waitForSelector(ready, { timeout: 30000 })
        .catch(() => problems.push(`${label}: never showed ${ready}`));
      // Let transitions and the first live figures settle.
      await page.waitForTimeout(400);

      if (name === 'board') {
        const applied = await page.evaluate(() => ({
          attribute: document.documentElement.dataset.theme,
          ground: getComputedStyle(document.body).backgroundColor,
          chrome: [...document.querySelectorAll('meta[name="theme-color"]')].map(m => m.content),
        }));
        const want = theme === 'light' ? 'rgb(242, 244, 247)' : 'rgb(23, 27, 33)';
        const wantChrome = theme === 'light' ? '#f2f4f7' : '#171b21';
        if (applied.attribute !== theme || applied.ground !== want) {
          problems.push(`${label}: chose ${theme} on a ${device} device, got ${JSON.stringify(applied)}`);
        }
        if (!applied.chrome.every(c => c === wantChrome)) {
          problems.push(`${label}: theme-color is ${applied.chrome}, not ${wantChrome}`);
        }
      }

      const { violations } = await new AxeBuilder({ page }).withTags(TAGS).analyze();
      for (const v of violations) {
        const where = v.nodes.slice(0, 3).map(n => n.target.join(' ')).join(' | ');
        problems.push(`${label}: ${v.id} (${v.impact}) ${v.nodes.length}x: ${where}`);
      }
      const sideways = await page.evaluate(() =>
        document.documentElement.scrollWidth - document.documentElement.clientWidth);
      if (sideways > 0) problems.push(`${label}: scrolls sideways by ${sideways}px`);
      await page.screenshot({ path: `${SHOTS}/${size}-${theme}-${name}.png`, fullPage: true });
      console.log(`${label.padEnd(24)}: ${violations.length ? `${violations.length} violation(s)` : 'clean'}`);
    }
    await ctx.close();
  }
}

// What axe cannot see, once.
const ctx = await browser.newContext({ ...devices['iPhone 14'], storageState: session });
const page = await ctx.newPage();
page.on('pageerror', e => problems.push(`keyboard: ${e.message}`));

// A chart reads out a moment at a time from the keyboard.
await page.goto(`${BASE}/host`, { waitUntil: 'networkidle' });
const chart = page.locator('.chart [role=slider]').first();
await chart.waitFor({ timeout: 30000 });
await chart.focus();
await page.keyboard.press('Home');
const oldest = await chart.getAttribute('aria-valuenow');
await page.keyboard.press('ArrowRight');
const next = await chart.getAttribute('aria-valuetext');
await page.keyboard.press('End');
const newest = await chart.getAttribute('aria-valuenow');
console.log('chart keys    :', `Home ${oldest}, End ${newest}; ${next}`);
if (oldest !== '0' || newest !== '100') problems.push(`chart keys: Home ${oldest}, End ${newest}`);

// A two-tap button: armed by the keyboard it keeps focus and says what the
// next press does, and disarms itself if nothing follows.
await page.goto(`${BASE}${stack}`, { waitUntil: 'networkidle' });
const takeDown = page.locator('button:has-text("Take down")');
await takeDown.focus();
await page.keyboard.press('Enter');
const armed = await page.evaluate(() => ({
  focused: document.activeElement?.textContent.trim(),
  said: document.querySelector('.confirm [role=status]')?.textContent.trim(),
}));
console.log('two taps      :', JSON.stringify(armed));
if (armed.focused !== 'Take it down') problems.push(`armed, focus is on "${armed.focused}"`);
if (!armed.said) problems.push('arming is not announced');
await page.waitForSelector('button:has-text("Take down")', { timeout: 8000 })
  .then(() => console.log('two taps      : disarmed by itself'),
    () => problems.push('an armed button did not disarm by itself'));
const deployed = await page.$$eval('.rows .row-name', els => els.map(e => e.textContent));
if (deployed.includes('Take down')) problems.push('arming from the keyboard took the stack down');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo accessibility violations found.');
process.exit(problems.length ? 1 : 0);
