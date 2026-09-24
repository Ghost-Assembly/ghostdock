// The board follows the host by itself. A container stopped and started
// from outside GhostDock, with the docker CLI, must show on an open board
// without anyone reloading it.
//
// Needs a live server with the Livebox fixture deployed. See run.sh.
import { execFileSync } from 'node:child_process';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const CONTAINER = 'livebox-box-1';

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

let loads = 0;
page.on('load', () => { loads += 1; });

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');

const row = '.row-link:has(.row-name:text("livebox")) .row-bar';
const state = () => page.$eval(row, el => el.dataset.state);
await page.waitForSelector(row, { timeout: 30000 });
console.log('board         :', `livebox ${await state()}`);
// Dragging across a row, as when trying to highlight it, must not open it;
// a plain click still does.
const box = await page.locator('.row-link:has(.row-name:text("livebox")) .row-name').boundingBox();
await page.mouse.move(box.x + 1, box.y + box.height / 2);
await page.mouse.down();
await page.mouse.move(box.x + box.width - 1, box.y + box.height / 2, { steps: 8 });
await page.mouse.up();
await page.waitForTimeout(300);
const stayed = new URL(page.url()).pathname === '/';
console.log('drag on a row :', stayed ? 'stayed on the board' : 'OPENED THE ROW');
if (!stayed) problems.push('dragging across a row opened it');

const loadsBefore = loads;

async function outside(verb, expected) {
  const t0 = Date.now();
  // `-t 1`: the sleeper ignores SIGTERM, and ten seconds of Docker's grace
  // period would be measured as GhostDock's latency.
  const args = verb === 'stop' ? ['stop', '-t', '1', CONTAINER] : [verb, CONTAINER];
  execFileSync('docker', args, { stdio: 'ignore' });
  await page.waitForFunction(
    want => [...document.querySelectorAll('.row-link')]
      .find(r => r.querySelector('.row-name')?.textContent === 'livebox')
      ?.querySelector('.row-bar')?.dataset.state === want,
    expected,
    { timeout: 20000 },
  );
  console.log(`docker ${verb.padEnd(6)}:`, `board says ${expected} after ${Date.now() - t0}ms`);
}

// A running stack's row carries its live figures.
await page.waitForFunction(() => [...document.querySelectorAll('.row-link')]
  .find(r => r.querySelector('.row-name')?.textContent === 'livebox')
  ?.querySelector('.row-figures')?.textContent.match(/cores, [\d.]+ (B|KiB|MiB|GiB)$/), null, { timeout: 30000 })
  .then(() => console.log('board figures : shown'), () => problems.push('the board row has no figures'));

await outside('stop', 'stopped');
await page.screenshot({ path: `${SHOTS}/live-stopped.png` });
await outside('start', 'running');

// The stack screen follows its own containers the same way.
await page.click('.row-link:has(.row-name:text("livebox"))');
await page.waitForSelector('h2.group-heading:text("Containers")');
execFileSync('docker', ['stop', '-t', '1', CONTAINER], { stdio: 'ignore' });
await page.waitForFunction(() =>
  [...document.querySelectorAll('.row-bar')].some(b => b.dataset.state === 'stopped'),
  null, { timeout: 20000 });
console.log('stack screen  : container shown stopped');
// The figure, not its area: an idle stack's CPU area has no height.
await page.waitForSelector('h2.group-heading:has-text("Resources") ~ .charts .chart', { timeout: 30000 })
  .then(() => console.log('stack charts  : drawn'), () => problems.push('the stack page has no resource charts'));
// Each container's now, typical and peak, beneath the stack's charts.
await page.waitForFunction(() => [...document.querySelectorAll('.rows-figures .row')]
  .some(r => r.querySelector('.row-name')?.textContent === 'livebox-box-1' && /typical .*peak /s.test(r.textContent)),
  null, { timeout: 30000 })
  .then(() => console.log('stack figures : typical and peak per container'), () => problems.push('the stack page has no per-container figures'));

if (loads !== loadsBefore) problems.push('the page reloaded; the updates must arrive by themselves');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no reloads.');
process.exit(problems.length ? 1 : 0);
