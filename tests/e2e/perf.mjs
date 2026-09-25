// Performance budget for the web client.
//
// The frontend is a Wasm bundle, which costs bytes that a server-rendered
// alternative would not. That trade was made deliberately, so it needs a
// number attached rather than an opinion: this fails the build when a cold
// load on a mediocre mobile connection stops being usable.
//
// Conditions model the case the product exists for -- something broke, you
// are not at a desk, and you are on a phone on mobile data.
//
// Run against a live server: `just test-perf`.
//
// Plain ESM rather than TypeScript, deliberately: these are three short linear
// scripts run directly by `node` with no build step, and the only types
// involved belong to Playwright. A compile step here would add a toolchain to
// the repository for no checking it does not already get.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';

/** Cold, interactive, on poor LTE with a mid-range CPU. */
const COLD_INTERACTIVE_MS = 3000;
/**
 * Bytes actually crossing the wire, so it also catches compression breaking
 * (uncompressed, the bundle is over three times this). The time above is
 * what decides usability; this is its early warning. Raised from 400 KB
 * when the theme, icons and API reference took the bundle to 411 KB with
 * the cold load still under 2.7 s.
 */
const TRANSFER_BUDGET_KB = 480;
/** A repeat visit should be effectively instant; this is the daily case. */
const WARM_INTERACTIVE_MS = 800;

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'] });
const page = await ctx.newPage();
const cdp = await ctx.newCDPSession(page);
await cdp.send('Network.enable');
await cdp.send('Network.emulateNetworkConditions', {
  offline: false,
  downloadThroughput: 1.6e6 / 8,
  uploadThroughput: 8e5 / 8,
  latency: 150,
});
await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });

// The moment the field appears, taken inside the page. Waiting for it from
// outside is not a measurement: Playwright polls every 500 ms once a wait
// runs long, so a load that got 30 ms slower could read 500 ms slower.
await page.addInitScript(() => {
  const seen = () => {
    if (window.__interactive === undefined && document.querySelector('input[type=password]'))
      window.__interactive = performance.now();
  };
  new MutationObserver(seen).observe(document, { childList: true, subtree: true });
});

async function load() {
  await page.goto(BASE, { waitUntil: 'commit' });
  // Usable means there is a field to type into, not that bytes arrived.
  await page.waitForSelector('input[type=password]', { timeout: 60000 });
  // Since navigation started, which is what the person waits through.
  const ms = Math.round(await page.evaluate(() => window.__interactive));
  const kb = await page.evaluate(() =>
    Math.round(
      performance.getEntriesByType('resource')
        .reduce((total, r) => total + (r.transferSize || 0), 0) / 1024,
    ));
  return { ms, kb };
}

// Median of several warm loads rather than one sample. A single noisy
// measurement -- a build running alongside, say -- would fail the budget for
// reasons that have nothing to do with the bundle, and a flaky budget gets
// ignored rather than fixed.
const cold = await load();

// One untimed load before the warm samples. The first repeat visits can
// still recompile the wasm, because the browser only caches compiled code
// once the module has tiered up in the background; on a busy machine under
// 4x CPU throttling that intermittently added ~700ms. A real repeat visit
// comes hours later, by which point the browser has persisted that cache,
// so warming it first measures what a returning person actually sees.
// This cannot hide a regression: the cold budget above still measures the
// full first-visit cost, including every byte of the bundle.
await load();

const warmSamples = [];
for (let i = 0; i < 3; i += 1) warmSamples.push((await load()).ms);
warmSamples.sort((a, b) => a - b);
const warm = { ms: warmSamples[1], kb: 0 };
await browser.close();

const failures = [];
if (cold.ms > COLD_INTERACTIVE_MS)
  failures.push(`cold interactive ${cold.ms}ms exceeds ${COLD_INTERACTIVE_MS}ms`);
if (cold.kb > TRANSFER_BUDGET_KB)
  failures.push(`transferred ${cold.kb}KB exceeds ${TRANSFER_BUDGET_KB}KB (is compression on?)`);
if (warm.ms > WARM_INTERACTIVE_MS)
  failures.push(`warm interactive ${warm.ms}ms exceeds ${WARM_INTERACTIVE_MS}ms`);

console.log(`cold  ${String(cold.ms).padStart(5)}ms  ${String(cold.kb).padStart(4)}KB   (budget ${COLD_INTERACTIVE_MS}ms / ${TRANSFER_BUDGET_KB}KB)`);
console.log(`warm  ${String(warm.ms).padStart(5)}ms  median of ${warmSamples.join(', ')}   (budget ${WARM_INTERACTIVE_MS}ms)`);

if (failures.length) {
  console.log(`\nOVER BUDGET:\n- ${failures.join('\n- ')}`);
  process.exit(1);
}
console.log('\nWithin budget.');
