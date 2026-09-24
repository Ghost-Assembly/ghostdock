// A container's logs: search, errors only, download, and following new
// output live until the container stops.
//
// Needs a live server with the Ticker fixture deployed. See run.sh.
import { execFileSync } from 'node:child_process';
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
await page.click('.row-link:has(.row-name:text("ticker"))', { timeout: 30000 });
await page.click('a.row-link:has(.row-count:text("Logs"))', { timeout: 20000 });
await page.waitForSelector('pre.log .log-line');

const texts = () => page.$$eval('pre.log .log-line', els => els.map(e => e.textContent.trim()));
const ticks = async () => (await texts()).filter(t => t.startsWith('tick ')).map(t => Number(t.slice(5)));
const snapshot = await ticks();
console.log('snapshot      :', `${snapshot.length} ticks, last ${snapshot.at(-1)}`);

// Search and errors-only narrow what is shown.
await page.fill('input[type=search]', 'tick 1');
const searched = await texts();
if (!searched.length || !searched.every(t => t.includes('tick 1'))) problems.push(`search shows ${searched}`);
await page.fill('input[type=search]', '');
await page.click('button:has-text("Errors only")');
const errs = await page.$$eval('pre.log .log-line', els => els.map(e => [e.textContent.trim(), e.classList.contains('log-stderr')]));
if (!errs.length || !errs.every(([t, stderr]) => t.startsWith('warn ') && stderr)) problems.push('errors-only shows stdout');
await page.click('button:has-text("Showing errors")');
console.log('filters       :', `search ${searched.length} lines, errors-only ${errs.length} lines`);

// The download is the same output, as a file.
const href = await page.getAttribute('a:has-text("Download")', 'href');
const file = await (await page.request.get(`${BASE}${href}`)).text();
if (!file.includes('tick 0')) problems.push('the download does not hold the output');

// Follow: new lines arrive, and none repeats where snapshot meets stream.
await page.click('button:has-text("Follow")');
const target = snapshot.at(-1) + 4;
await page.waitForFunction(t => [...document.querySelectorAll('pre.log .log-line')]
  .some(e => e.textContent.trim() === `tick ${t}`), target, { timeout: 20000 });
const followed = await ticks();
const dupes = followed.filter((n, i) => followed.indexOf(n) !== i);
const gaps = followed.slice(1).filter((n, i) => n !== followed[i] + 1);
console.log('followed      :', `up to tick ${followed.at(-1)}, ${dupes.length} repeated, ${gaps.length} gaps`);
if (dupes.length) problems.push(`lines repeated where the snapshot met the stream: ${dupes}`);
if (gaps.length) problems.push(`lines missing where the snapshot met the stream: ${gaps}`);
const pinned = await page.$eval('pre.log', el => el.scrollHeight - el.scrollTop - el.clientHeight);
if (pinned > 60) problems.push(`the view did not keep up with new lines (${pinned}px from the bottom)`);
await page.screenshot({ path: `${SHOTS}/logs-following.png` });

// Stopping the container ends following, with a reason.
execFileSync('docker', ['stop', '-t', '1', 'ticker-ticker-1'], { stdio: 'ignore' });
await page.waitForSelector('text=The container stopped', { timeout: 20000 }).catch(async () => {
  const note = await page.$eval('.entry-note', el => el.textContent).catch(() => 'no note');
  problems.push(`following did not end with the container; note: ${note}`);
});
console.log('after stop    :', (await page.textContent('button[aria-pressed]')).trim());

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors.');
process.exit(problems.length ? 1 : 0);
