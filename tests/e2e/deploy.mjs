// The deploy path, end to end in a real browser.
//
// Registers a stack, deploys it while watching output stream in, then does
// the same with a stack that comes up broken and checks the failure is
// reported with compose's own words. Finally takes the broken one down.
//
// Needs a live server with a FRESH database and a reachable Docker daemon.
// See `just test-deploy`.
//
// Plain ESM rather than TypeScript, deliberately: these are three short linear
// scripts run directly by `node` with no build step, and the only types
// involved belong to Playwright. A compile step here would add a toolchain to
// the repository for no checking it does not already get.
import { execFileSync } from 'node:child_process';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const PASSWORD = 'correct horse battery staple';

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
let expect4xx = false;
page.on('console', m => {
  if (m.type() !== 'error') return;
  if (expect4xx && m.text().includes('Failed to load resource')) return;
  problems.push(`console: ${m.text()}`);
});
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

const HEALTHY = `services:
  web:
    image: alpine:3.22
    command: ["sh","-c","while :; do sleep 3600; done"]
    healthcheck:
      test: ["CMD","true"]
      interval: 1s
      retries: 2`;
const BROKEN = `services:
  sick:
    image: alpine:3.22
    command: ["sh","-c","while :; do sleep 3600; done"]
    healthcheck:
      test: ["CMD","false"]
      interval: 1s
      timeout: 1s
      retries: 1`;

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.entry-heading');
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', PASSWORD);
await page.click('button[type=submit]');
// A fresh instance has no stacks, so the board shows its empty state rather
// than a verdict line.
await page.waitForSelector('.verdict-line, .state-note', { timeout: 30000 });
console.log('signed in     :', (await page.textContent('.state-note, .verdict-line')).trim().split('\n')[0]);

async function createStack(name, yaml) {
  await page.click('.topbar a:has-text("New stack")');
  await page.waitForSelector('textarea');
  await page.fill('input[type=text]', name);
  await page.fill('textarea', yaml);
  await page.click('button[type=submit]');
  await page.waitForSelector('.actions', { timeout: 20000 });
  return new URL(page.url()).pathname;
}

// 1. Create and deploy a healthy stack, watching live output arrive.
const path1 = await createStack('Demo App', HEALTHY);
console.log('created       :', path1, '·', await page.textContent('h1.wordmark'));
await page.click('button:has-text("Deploy")');
await page.waitForSelector('pre.log', { timeout: 30000 });
const firstLine = (await page.textContent('pre.log')).split('\n')[0];
console.log('live output   :', JSON.stringify(firstLine.slice(0, 60)));
await page.screenshot({ path: `${SHOTS}/deploying.png` });

await page.waitForSelector('.rows .row-detail:has-text("succeeded")', { timeout: 120000 });
console.log('history       :', (await page.textContent('.rows .row-detail')).trim());
await page.screenshot({ path: `${SHOTS}/deployed.png` });

// 2. The board should now show it running, and be tappable.
await page.click('.nav-item:has-text("Stacks")');
await page.waitForSelector('.verdict-line');
console.log('board verdict :', await page.textContent('.verdict-line'));
const rows = await page.$$eval('.row-link', els => els.map(e =>
  `${e.querySelector('.row-name').textContent}=${e.querySelector('.row-bar').dataset.state}` +
  `${e.tagName === 'A' ? ' [tappable]' : ''}`));
console.log('board rows    :', rows.join(', '));

// 3. A broken stack must report failure, with compose's own words.
const path2 = await createStack('Broken', BROKEN);
await page.click('button:has-text("Deploy")');
await page.waitForSelector('.rows .row-detail:has-text("failed")', { timeout: 120000 });
const brokenResult = (await page.textContent('.rows .row-detail')).trim();
console.log('broken result :', brokenResult);
if (!brokenResult.includes('failed')) {
  problems.push(`a stack that comes up unhealthy must report failure, got: ${brokenResult}`);
}
await page.click('.rows a.row-link');
await page.waitForSelector('pre.log', { timeout: 20000 });
console.log('verdict       :', await page.textContent('.verdict-line'));
const log = await page.textContent('pre.log');
const lastLine = log.trim().split('\n').pop();
console.log('compose said  :', JSON.stringify(lastLine.slice(0, 70)));
if (!lastLine.includes('unhealthy')) {
  problems.push(`compose's own explanation should reach the user, got: ${lastLine}`);
}
await page.screenshot({ path: `${SHOTS}/failure.png` });

// 4. Taking a stack down removes its containers but keeps the registration.
// The first tap only arms the button; nothing may happen until the second.
await page.goto(`${BASE}${path2}`);
await page.waitForSelector('button:has-text("Take down")');
await page.click('button:has-text("Take down")');
await page.waitForTimeout(500);
const armedHistory = await page.$$eval('.rows .row-name', els => els.map(e => e.textContent));
if (armedHistory.includes('Take down')) problems.push('one tap took the stack down; it must take two');
await page.click('button:has-text("Take it down")');
await page.waitForSelector('.rows .row-link:has(.row-name:text("Take down")) .row-detail:has-text("succeeded")',
  { timeout: 60000 });
await page.waitForSelector('text=Nothing is running for this stack.', { timeout: 20000 });
console.log('taken down    :', (await page.textContent('h1.wordmark')).trim(), 'still registered, no containers');

// 5. Leaving at once does not stop what was asked for: deploy, then go
// straight to another screen without waiting for any answer.
await page.goto(`${BASE}${path1}`);
await page.waitForSelector('button:has-text("Deploy"):not([disabled])');
const before = await page.evaluate(async id => (await (await fetch(`/api/v1/stacks/${id}/deployments`)).json()).length,
  path1.split('/').pop());
await page.click('button:has-text("Deploy")');
await page.click('.nav-item:has-text("Settings")');
let outcome = 'none';
for (let i = 0; i < 90 && !['succeeded', 'failed'].includes(outcome); i++) {
  await page.waitForTimeout(1000);
  const list = await page.evaluate(async id => (await fetch(`/api/v1/stacks/${id}/deployments`)).json(), path1.split('/').pop());
  outcome = list.length > before ? list[0].status : 'not started';
}
console.log('left at once  :', `deploy ${outcome}`);
if (outcome !== 'succeeded') problems.push(`leaving the screen stopped the deploy: ${outcome}`);

// 6. Busy is what the server says is running. A read that found an
// operation running, whose ending was then missed, must not leave the
// buttons saying "Working" once a later read finds nothing running. The
// first read is made to say so; the container restarting makes it look again.
let faked = false;
await page.route('**/api/v1/stacks/*/deployments', async route => {
  if (faked) return route.continue();
  faked = true;
  const response = await route.fetch();
  const list = await response.json();
  if (list[0]) list[0].status = 'running';
  await route.fulfill({ response, json: list });
});
await page.goto(`${BASE}${path1}`);
await page.waitForSelector('button:has-text("Working")', { timeout: 20000 });
await page.waitForTimeout(1000);
execFileSync('docker', ['restart', '-t', '0', 'demo-app-web-1'], { stdio: 'ignore' });
const cleared = await page.waitForSelector('button:has-text("Deploy"):not([disabled])', { timeout: 20000 })
  .then(() => true, () => false);
console.log('busy          :', cleared ? 'cleared once nothing was running' : 'STUCK on Working');
if (!cleared) problems.push('the stack stayed busy after a read found nothing running');
await page.unroute('**/api/v1/stacks/*/deployments');

const overflow = await page.evaluate(() =>
  document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`page scrolls horizontally by ${overflow}px`);

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow.');
process.exit(problems.length ? 1 : 0);
