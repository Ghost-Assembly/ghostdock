// Many tabs at once, on a desktop. Over plain HTTP a browser allows six
// connections per host across all tabs; when each tab held one open for
// live events, the sixth froze with its requests queued. Every tab here
// must load. The live connection must also come back after the server
// closes it, and carry on delivering.
//
// Needs a live server with the Livebox fixture deployed. See run.sh.
import { execFileSync } from 'node:child_process';
import { chromium } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const TABS = 8;

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const problems = [];

const first = await ctx.newPage();
first.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
await first.goto(BASE, { waitUntil: 'networkidle' });
await first.fill('input[type=text]', 'admin');
await first.fill('input[type=password]', 'correct horse battery staple');
await first.click('button[type=submit]');
await first.waitForSelector('.row-link', { timeout: 30000 });

for (let n = 2; n <= TABS; n++) {
  const tab = await ctx.newPage();
  const t0 = Date.now();
  await tab.goto(`${BASE}/settings`, { timeout: 10000 }).catch(() => {});
  const loaded = await tab.waitForSelector('.row-detail:has-text("Docker")', { timeout: 8000 }).then(() => true, () => false);
  if (!loaded) problems.push(`tab ${n} never received its data`);
  if (n === TABS) console.log(`tab ${n}       :`, loaded ? `loaded in ${Date.now() - t0}ms` : 'FROZEN');
}

// The server closes the first tab's socket when its password changes.
const changed = await first.evaluate(async () => (await fetch('/api/v1/auth/password', {
  method: 'PUT',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ current: 'correct horse battery staple', new: 'a different long passphrase' }),
  // A frozen tab queues this forever; fail instead of hanging.
  signal: AbortSignal.timeout(10000),
})).status).catch(e => `no answer (${e.message})`);
if (changed !== 204) problems.push(`password change returned ${changed}`);
await first.waitForTimeout(2500);

// Reconnected with its new session, it still hears about the host.
execFileSync('docker', ['stop', '-t', '0', 'livebox-box-1'], { stdio: 'ignore' });
const heard = await first.waitForFunction(() => [...document.querySelectorAll('.row-link')]
  .find(r => r.querySelector('.row-name')?.textContent === 'livebox')
  ?.querySelector('.row-bar')?.dataset.state === 'stopped', null, { timeout: 15000 }).then(() => true, () => false);
console.log('reconnected   :', heard ? 'live updates resumed' : 'NO live updates');
if (!heard) problems.push('live updates did not resume after the server closed the socket');

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : `\nAll ${TABS} tabs loaded.`);
process.exit(problems.length ? 1 : 0);
