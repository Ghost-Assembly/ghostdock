// The layout at both sizes it is designed for. On a desktop: navigation in
// a sidebar, the board in columns, a stack's page in two panes side by
// side. On a phone: a bottom bar and one column. Neither scrolls sideways.
// Screenshots of each screen land in target/e2e-shots/{desktop,phone}.
//
// Needs a live server with the Livebox fixture and some stacks. See run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const problems = [];
const browser = await chromium.launch();

async function signedIn(options) {
  const ctx = await browser.newContext({ ...options, colorScheme: 'dark' });
  const page = await ctx.newPage();
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  await page.goto(BASE, { waitUntil: 'networkidle' });
  await page.fill('input[type=text]', 'admin');
  await page.fill('input[type=password]', 'correct horse battery staple');
  await page.click('button[type=submit]');
  await page.waitForSelector('.row-link', { timeout: 30000 });
  return page;
}

const overflow = page => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
const box = (page, sel) => page.locator(sel).first().boundingBox();
// Both panes read in one go, once the page has its content: measured
// separately, one could move between the two readings.
const panes = async page => {
  await page.waitForSelector('a.row-link:has(.row-count:text("Logs"))');
  await page.waitForTimeout(300);
  return page.evaluate(() => {
    const r = sel => document.querySelector(sel).getBoundingClientRect();
    const [main, side] = [r('.detail-main'), r('.detail-side')];
    return { main: { x: main.x, y: main.y, width: main.width, height: main.height }, side: { x: side.x, y: side.y } };
  });
};

async function tour(page, dir, checks) {
  const shot = async name => { await page.waitForTimeout(600); await page.screenshot({ path: `${SHOTS}/${dir}/${name}.png`, fullPage: true }); };
  await page.goto(`${BASE}/`);
  await page.waitForSelector('.row-link');
  await checks.board(page);
  await shot('board');
  const stack = await page.getAttribute('.row-link:has(.row-name:text("livebox"))', 'href');
  await page.goto(`${BASE}${stack}`);
  await page.waitForSelector('.detail-side .group-heading');
  await checks.stack(page);
  await shot('stack');
  const logs = await page.getAttribute('a.row-link:has(.row-count:text("Logs"))', 'href');
  for (const [name, path] of [['logs', logs], ['settings', '/settings'], ['updates', '/updates'],
    ['new-stack', '/stacks/new'], ['tokens', '/tokens'], ['sources', '/sources'], ['cleanup', '/cleanup'], ['host', '/host']]) {
    await page.goto(`${BASE}${path}`);
    await page.waitForSelector('h1.wordmark');
    const sideways = await overflow(page);
    if (sideways > 0) problems.push(`${dir} ${name} scrolls sideways by ${sideways}px`);
    await shot(name);
  }
}

const desktop = await signedIn({ viewport: { width: 1440, height: 900 } });
await tour(desktop, 'desktop', {
  async board(page) {
    const nav = await box(page, '.nav');
    const main = await box(page, '.shell');
    console.log('desktop nav   :', `${Math.round(nav.width)}x${Math.round(nav.height)} at x=${Math.round(nav.x)}`);
    if (!(nav.x === 0 && nav.height > 600 && nav.width < 300)) problems.push('desktop navigation is not a left sidebar');
    if (main.x < nav.width) problems.push('desktop content sits under the sidebar');
    const lefts = new Set(await page.$$eval('.rows-board .row', rows => rows.map(r => Math.round(r.getBoundingClientRect().left))));
    console.log('desktop board :', `${lefts.size} columns`);
    if (lefts.size < 2) problems.push('the desktop board is a single column');
    // A running stack's CPU trend has room on a desktop.
    await page.waitForSelector('.rows-board .row-link .sparkline', { timeout: 20000 })
      .then(() => console.log('desktop trend : shown'), () => problems.push('no CPU trend on the desktop board'));
  },
  async stack(page) {
    const { main, side } = await panes(page);
    const sideBySide = side.x > main.x + main.width - 1 && Math.abs(side.y - main.y) < 40;
    console.log('desktop stack :', sideBySide ? 'two panes side by side' : 'NOT side by side');
    if (!sideBySide) problems.push('the desktop stack page is not two panes');
  },
});

const phone = await signedIn({ ...devices['iPhone 14'] });
await tour(phone, 'phone', {
  async board(page) {
    const nav = await box(page, '.nav');
    const vp = page.viewportSize();
    const bottom = Math.round(nav.y + nav.height) >= vp.height - 1 && nav.width >= vp.width - 1;
    console.log('phone nav     :', bottom ? 'bottom bar' : 'NOT a bottom bar');
    if (!bottom) problems.push('phone navigation is not a bottom bar');
    if (await page.isVisible('.nav-brand')) problems.push('the sidebar wordmark shows on a phone');
    // A phone keeps the figures and leaves the trend out.
    await page.waitForSelector('.rows-board .row-figures', { timeout: 20000 });
    if (await page.isVisible('.rows-board .sparkline')) problems.push('the CPU trend shows on a phone');
  },
  async stack(page) {
    const { main, side } = await panes(page);
    const stacked = side.y >= main.y + main.height - 1;
    console.log('phone stack   :', stacked ? 'one column' : 'NOT one column');
    if (!stacked) problems.push('the phone stack page is not one column');
  },
});

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nBoth layouts hold.');
process.exit(problems.length ? 1 : 0);
