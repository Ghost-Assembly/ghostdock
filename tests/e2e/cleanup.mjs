// Removing stopped containers nothing needs, from a preview.
//
// This runs on a real host, so it removes only what it made: a group is
// removed only when it lists exactly this test's own containers, and the
// image buttons are never pressed. See run.sh for the fixtures.
import { execFileSync } from 'node:child_process';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const GONE = 'ghostdock-e2e-gone-web-1';
const BY_HAND = 'ghostdock-e2e-by-hand';

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
await page.click('a:has-text("Reclaim disk space")');
await page.locator('.verdict-line').or(page.getByText('Nothing to reclaim')).first().waitFor({ timeout: 30000 });

const group = heading => page.evaluate(h => {
  const title = [...document.querySelectorAll('h2.group-heading')].find(e => e.textContent.trim() === h);
  const list = title?.nextElementSibling;
  return list ? [...list.querySelectorAll('.row-name')].map(e => e.textContent.trim()) : [];
}, heading);

const leftover = await group('Left by stacks that are gone');
const standalone = await group('Stopped, not from Compose');
console.log('left over     :', leftover.join(', ') || '(none)');
console.log('by hand       :', standalone.join(', ') || '(none)');
await page.screenshot({ path: `${SHOTS}/cleanup.png`, fullPage: true });

async function removeGroup(names, mine, button) {
  if (!names.includes(mine)) {
    problems.push(`${mine} was not offered`);
    return;
  }
  if (names.length !== 1) {
    // Something else on this host would be removed too. Not this test's call.
    console.log('skipped       :', `${button}: the host has other stopped containers`);
    return;
  }
  await page.click(`button:has-text("${button}")`);
  // Gone from the preview, which is re-read after removing. The result
  // line cannot be waited on: the previous removal left the same words.
  await page.waitForFunction(
    name => ![...document.querySelectorAll('.row-name')].some(e => e.textContent.trim() === name),
    mine, { timeout: 20000 });
  const exists = (() => {
    try { execFileSync('docker', ['inspect', mine], { stdio: 'ignore' }); return true; } catch { return false; }
  })();
  if (exists) problems.push(`${mine} still exists after removal`);
  console.log('removed       :', mine);
}

await removeGroup(leftover, GONE, 'left over');
await removeGroup(standalone, BY_HAND, 'stopped');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors.');
process.exit(problems.length ? 1 : 0);
