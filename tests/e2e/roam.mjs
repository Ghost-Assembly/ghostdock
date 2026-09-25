// Moving through every screen quickly, on a desktop-sized window, while
// live events arrive. A panic anywhere kills the client outright (release
// builds abort), leaving a page that looks alive and does nothing, so any
// panic or page error fails this test.
//
// Needs a live server with the Livebox fixture deployed. See run.sh.
import { execFile } from 'node:child_process';
import { chromium } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const ROUNDS = Number(process.env.ROUNDS ?? 6);
// Real links are slower than loopback. With every request taking this long,
// screens are routinely left while their requests are still in flight,
// which is where a screen reading its own state after it has gone breaks.
const LATENCY_MS = Number(process.env.LATENCY_MS ?? 250);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => {
  const t = m.text();
  if (m.type() === 'error' && !t.includes('Failed to load resource')) problems.push(`console: ${t}`);
});
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));

// Every socket the page opens, and whether it has closed. A screen that
// holds one must close it when it goes: an open shell socket is a shell
// still running on the server.
const sockets = [];
page.on('websocket', ws => {
  const entry = { url: ws.url(), closed: false };
  sockets.push(entry);
  ws.on('close', () => { entry.closed = true; });
});
const openShells = () => sockets.filter(s => s.url.includes('/exec') && !s.closed).length;
async function shellsClosed() {
  for (let i = 0; i < 50 && openShells() > 0; i++) await page.waitForTimeout(100);
  return openShells() === 0;
}

const cdp = await ctx.newCDPSession(page);
await cdp.send('Network.enable');
await cdp.send('Network.emulateNetworkConditions', {
  offline: false, latency: LATENCY_MS, downloadThroughput: -1, uploadThroughput: -1,
});

await page.goto(BASE, { waitUntil: 'networkidle' });
await page.fill('input[type=text]', 'admin');
await page.fill('input[type=password]', 'correct horse battery staple');
await page.click('button[type=submit]');
await page.waitForSelector('.nav-item', { timeout: 30000 });

// Events in the background: the container stops and starts throughout.
let churning = true;
const churn = (async () => {
  for (let i = 0; churning; i++) {
    await new Promise(r => execFile('docker', [i % 2 ? 'start' : 'stop', '-t', '0', 'livebox-box-1'], r));
    await new Promise(r => setTimeout(r, 400));
  }
})();

const stackHref = await page.getAttribute('.row-link:has(.row-name:text("livebox"))', 'href', { timeout: 20000 });
await page.goto(`${BASE}${stackHref}`);
const containerLogs = await page.getAttribute('a.row-link:has(.row-count:text("Logs"))', 'href', { timeout: 20000 });
const containerShell = containerLogs.replace('/logs', '/shell');

const routes = [
  '/', stackHref, `${stackHref}/env`, `${stackHref}/edit`, '/updates', '/settings', '/sources',
  '/cleanup', '/activity', '/accounts', '/tokens', containerLogs, containerShell, '/stacks/new',
];

for (let round = 0; round < ROUNDS; round++) {
  for (const route of routes) {
    // Client-side navigation, as a person clicking would, with no pause for
    // the screen to finish loading: leaving mid-request is the point.
    await page.evaluate(r => {
      history.pushState(null, '', r);
      dispatchEvent(new PopStateEvent('popstate'));
    }, route);
    await page.waitForTimeout(round % 2 ? 30 : 150);
  }
  for (let i = 0; i < 5; i++) { await page.goBack(); await page.waitForTimeout(40); }
  for (let i = 0; i < 3; i++) { await page.goForward(); await page.waitForTimeout(40); }
}
churning = false;
await churn;

// Leaving the shell closes its socket. Checked with the container running
// and nothing else closing it: the server ends a shell whose container
// stops, which would otherwise hide a screen that never let go.
await new Promise(r => execFile('docker', ['start', 'livebox-box-1'], r));
const go = r => page.evaluate(to => { history.pushState(null, '', to); dispatchEvent(new PopStateEvent('popstate')); }, r);
await go(containerShell);
await page.waitForFunction(() => !document.querySelector('button[type=submit]')?.disabled, null, { timeout: 20000 });
await go('/settings');
const closed = await shellsClosed();
console.log('shell sockets :', `${sockets.filter(s => s.url.includes('/exec')).length} opened, ${closed ? 'all closed on leaving' : `${openShells()} LEFT OPEN`}`);
if (!closed) problems.push('leaving the shell left its socket open');

// Still alive? A dead client cannot navigate or render.
await page.evaluate(() => { history.pushState(null, '', '/settings'); dispatchEvent(new PopStateEvent('popstate')); });
const alive = await page.waitForSelector('h1.wordmark:has-text("Settings")', { timeout: 5000 }).then(() => true, () => false);
console.log('after roaming :', alive ? 'responsive' : 'FROZEN');
if (!alive) problems.push('the client stopped responding');

await browser.close();
const unique = [...new Set(problems)];
console.log(unique.length ? `\nPROBLEMS:\n- ${unique.slice(0, 15).join('\n- ')}` : '\nNo panics, no errors.');
process.exit(unique.length ? 1 : 0);
