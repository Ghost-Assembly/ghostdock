// The installed app: manifest and icons are served, the app itself opens
// with the server gone, and nothing from the API is ever kept on the device.
//
// 127.0.0.1 counts as a secure context, so the service worker registers
// here as it would behind HTTPS. Needs a fresh server. See run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['Pixel 7'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

// 1. What an install needs.
const manifest = await (await page.request.get(`${BASE}/manifest.webmanifest`)).json();
for (const icon of manifest.icons) {
  const res = await page.request.get(`${BASE}${icon.src}`);
  if (!res.ok()) problems.push(`icon ${icon.src} is ${res.status()}`);
}
const sizes = manifest.icons.map(i => `${i.sizes}/${i.purpose}`);
console.log('manifest      :', sizes.join(', '));
for (const need of ['192x192/any', '512x512/any', '512x512/maskable']) {
  if (!sizes.includes(need)) problems.push(`no ${need} icon`);
}

// 2. Sign in so there is something private in play, then let the worker
// take control.
await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.waitForSelector('.nav-item', { timeout: 30000 });
await page.evaluate(() => navigator.serviceWorker.ready);
await page.reload({ waitUntil: 'networkidle' });
const controlled = await page.evaluate(() => !!navigator.serviceWorker.controller);
console.log('worker        :', controlled ? 'controls the page' : 'NOT in control');
if (!controlled) problems.push('the service worker did not take control');

// 3. Nothing from the API was stored.
const cached = await page.evaluate(async () => {
  const urls = [];
  for (const name of await caches.keys()) {
    for (const req of await (await caches.open(name)).keys()) urls.push(new URL(req.url).pathname);
  }
  return urls;
});
console.log('cached        :', cached.sort().join(' '));
if (cached.some(u => u.startsWith('/api/'))) problems.push('an API response was cached');
if (!cached.some(u => u.endsWith('.wasm'))) problems.push('the app itself was not cached');
// The sign-in screen draws no icons, so the page never asked for them; the
// worker keeps them anyway, or an app opened offline has blank buttons.
if (!cached.includes('/icons/lucide.svg')) problems.push('the icons were not kept for offline use');

// 4. Offline, the app still opens, says why it cannot show anything, and
// recovers by itself when the connection returns.
await ctx.setOffline(true);
await page.reload();
await page.waitForSelector('h1.entry-heading:has-text("not responding")', { timeout: 15000 });
console.log('offline       :', await page.textContent('h1.entry-heading'));
await page.screenshot({ path: `${SHOTS}/offline.png` });
await ctx.setOffline(false);
await page.waitForSelector('.nav-item', { timeout: 15000 });
console.log('back online   : signed-in board, no reload');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nOffline shell works, API never cached.');
process.exit(problems.length ? 1 : 0);
