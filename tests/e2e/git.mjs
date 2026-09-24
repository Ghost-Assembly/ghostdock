// Registering and deploying a Git-backed stack, in a real browser.
//
// Needs a live server with a FRESH database, a Docker daemon, and a git
// repository whose path is given in GHOSTDOCK_TEST_REPO.
//
// Plain ESM rather than TypeScript, deliberately: these are short linear
// scripts run directly by `node` with no build step, and the only types
// involved belong to Playwright.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const REPO = process.env.GHOSTDOCK_TEST_REPO;
const PASSWORD = 'correct horse battery staple';
const SECRET = 'hello from a sealed value';

if (!REPO) {
  console.error('GHOSTDOCK_TEST_REPO must point at a git repository');
  process.exit(2);
}

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
page.on('console', m => { if (m.type() === 'error' && !m.text().includes('Failed to load resource')) problems.push(`console: ${m.text()}`); });

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.waitForSelector('h1.entry-heading');
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', PASSWORD);
await page.click('button[type=submit]');
await page.waitForSelector('.verdict-line, .state-note');

// 1. Add a credential and a repository from Settings.
await page.click('.nav-item:has-text("Settings")');
await page.click('a[href="/sources"]');
await page.waitForSelector('h1.wordmark:has-text("Sources")');

await page.fill('input[placeholder="github"]', 'test-token');
await page.fill('input[type=password]', 'ghp_not_a_real_token');
await page.click('button:has-text("Add credential")');
await page.waitForSelector('.row-name:has-text("test-token")', { timeout: 15000 });
console.log('credential    : added, value never shown back');
if ((await page.content()).includes('ghp_not_a_real_token')) {
  problems.push('the token appears in the rendered page');
}

await page.fill('input[placeholder="https://github.com/you/stacks.git"]', `file://${REPO}`);
await page.click('button:has-text("Add repository")');
await page.waitForSelector(`.row-name:has-text("${REPO}")`, { timeout: 15000 });
console.log('repository    : added');
await page.screenshot({ path: `${SHOTS}/git-sources.png` });

// 2. Register a stack pointing into it.
await page.goto(`${BASE}/stacks/new/git`, { waitUntil: 'networkidle' });
await page.waitForSelector('select');
await page.fill('input[type=text] >> nth=0', 'Blog');
await page.fill('input[placeholder="compose/blog.yml"]', 'compose/blog.yml');
await page.click('button:has-text("Save stack")');
await page.waitForSelector('.actions', { timeout: 20000 });
const source = await page.textContent('.row-detail');
console.log('stack source  :', source.trim());

// 3. Give it a sealed variable.
await page.click('a:has-text("Edit variables")');
await page.waitForSelector('h1.wordmark:has-text("Environment")');
await page.fill('input[placeholder="API_KEY"]', 'GREETING');
await page.fill('input[type=password]', SECRET);
await page.click('button:has-text("Save variable")');
await page.waitForSelector('.row-name:has-text("GREETING")', { timeout: 15000 });
console.log('variable      : stored, listed by name only');
if ((await page.content()).includes(SECRET)) {
  problems.push('the variable value is rendered back into the page');
}
await page.screenshot({ path: `${SHOTS}/git-env.png` });

// 4. Deploy from the repository.
await page.click('a.topbar-link:has-text("Back")');
await page.waitForSelector('.actions');
await page.click('button:has-text("Deploy")');
await page.waitForSelector('.rows .row-detail:has-text("succeeded")', { timeout: 180000 });
console.log('deploy        : succeeded');
// The note under Source, found by what it says rather than by position:
// other sections put their own notes above it.
const commit = (await page.textContent('.entry-note:has-text("deployed")')).trim();
console.log('commit        :', commit);
if (!/^last deployed [0-9a-f]{7,}/.test(commit)) {
  problems.push(`the deployed commit should be recorded, got: ${commit}`);
}
await page.screenshot({ path: `${SHOTS}/git-deployed.png` });

const overflow = await page.evaluate(() =>
  document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`page scrolls horizontally by ${overflow}px`);

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow, no secrets rendered.');
process.exit(problems.length ? 1 : 0);
