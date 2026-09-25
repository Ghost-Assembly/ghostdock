// Uptime checks and alerts, end to end: a webhook channel that hears a
// test and never shows its URL again; an HTTP check against GhostDock's own
// health that comes up; a TCP check to a closed port that goes down with an
// incident, which the channel hears; and a board row marked when a stack's
// check is down.
//
// The webhook and the closed port are this test's own, on 127.0.0.1.
// Needs a live server with a FRESH database. See run.sh.
import { createServer } from 'node:http';
import { createServer as createTcpServer } from 'node:net';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const SECRET = 'e2e-hook-secret';

// A webhook that records what it is sent.
const heard = [];
const hook = createServer((req, res) => {
  let body = '';
  req.on('data', chunk => { body += chunk; });
  req.on('end', () => {
    heard.push({ path: req.url, body });
    res.end('ok');
  });
});
await new Promise(resolve => hook.listen(0, '127.0.0.1', resolve));
const hookHost = `127.0.0.1:${hook.address().port}`;

// A port nothing listens on: bound, then let go.
const holder = createTcpServer();
await new Promise(resolve => holder.listen(0, '127.0.0.1', resolve));
const closed = `127.0.0.1:${holder.address().port}`;
await new Promise(resolve => holder.close(resolve));

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
await page.waitForSelector('.nav-item', { timeout: 30000 });

const waitFor = async (what, check, timeout = 20000) => {
  const until = Date.now() + timeout;
  while (Date.now() < until) {
    if (await check()) return true;
    await page.waitForTimeout(200);
  }
  problems.push(`never: ${what}`);
  return false;
};

// 1. A channel: added, tested, and its URL never shown again.
await page.click('.nav-item:has-text("Settings")');
await page.click('a.row-link:has-text("Alerts")');
await page.waitForSelector('h1.wordmark:has-text("Alerts")');
const channelForm = page.locator('form', { has: page.locator('button:has-text("Add channel")') });
await channelForm.getByLabel('Name', { exact: true }).fill('e2e hook');
await channelForm.getByLabel('URL', { exact: true }).fill(`http://${hookHost}/hooks/${SECRET}`);
await channelForm.locator('button[type=submit]').click();
await page.waitForSelector(`.row:has(.row-name:text-is("e2e hook")) .row-detail:has-text("webhook to ${hookHost}")`, { timeout: 10000 })
  .catch(() => problems.push('the channel row does not name its host'));
const fieldLeft = await channelForm.getByLabel('URL', { exact: true }).inputValue();
if (fieldLeft) problems.push('the URL was left in its field after adding');
await page.click('button:has-text("Send test")');
await waitFor('the webhook hears the test', () => heard.some(h => JSON.parse(h.body).kind === 'test'));
await page.waitForSelector('[role=status]:has-text("Test delivered")', { timeout: 10000 })
  .catch(() => problems.push('the test is not said to be delivered'));
await page.waitForSelector('.row:has(.row-name:text-is("e2e hook test"))', { timeout: 10000 })
  .catch(() => problems.push('the delivery is not in the log'));
const shown = await page.content();
if (shown.includes(SECRET)) problems.push('the channel URL is on the page');
const listed = await (await page.request.get(`${BASE}/api/v1/alerts/channels`)).text();
if (listed.includes(SECRET)) problems.push('the channel URL comes back from the API');
console.log('channel       :', `test heard at ${heard[0]?.path.replace(SECRET, '…')}; URL shown nowhere`);
await page.screenshot({ path: `${SHOTS}/checks-alerts.png`, fullPage: true });

// 2. An HTTP check against GhostDock's own health comes up.
const addCheck = async (name, kind, target, retries) => {
  await page.click('.nav-item:has-text("Host")');
  await page.click('a.button:has-text("Add a check")');
  await page.waitForSelector('h1.wordmark:has-text("New check")');
  await page.getByLabel(/^Name/).fill(name);
  await page.getByLabel(/^Kind/).selectOption(kind);
  await page.getByLabel(/^Target/).fill(target);
  if (retries) await page.getByLabel('Down after this many failures in a row').fill(retries);
  await page.click('button:has-text("Add check")');
  await page.waitForURL(/\/checks\/\d+$/, { timeout: 10000 });
};
await addCheck('GhostDock itself', 'http', `${BASE}/api/v1/health`);
await page.waitForSelector('.rows .row-bar[data-state="running"] ~ .row-detail:has-text("up")', { timeout: 20000 })
  .catch(() => problems.push('the HTTP check never came up'));
const upText = await page.textContent('.rows .row-detail').catch(() => '');
console.log('http check    :', upText.trim());
await page.screenshot({ path: `${SHOTS}/checks-up.png`, fullPage: true });

// 3. A TCP check to a closed port goes down, with an incident, and the
// channel hears it.
await addCheck('Closed port', 'tcp', closed, '1');
await page.waitForSelector('.rows .row-bar[data-state="unhealthy"] ~ .row-detail:has-text("down")', { timeout: 20000 })
  .catch(() => problems.push('the TCP check never went down'));
await page.waitForSelector('h2:text("Incidents") + ul .row-detail:has-text("down now")', { timeout: 20000 })
  .catch(() => problems.push('no open incident'));
await waitFor('the webhook hears it go down', () => heard.some(h => {
  const alert = JSON.parse(h.body);
  return alert.kind === 'check' && alert.subject === 'Closed port' && alert.state === 'down';
}));
console.log('tcp check     :', (await page.textContent('.rows .row-detail').catch(() => '')).trim());
await page.screenshot({ path: `${SHOTS}/checks-down.png`, fullPage: true });

// 4. The Host screen lists both, each with its state as a mark and words.
await page.click('.nav-item:has-text("Host")');
await page.waitForSelector('.rows-checks .row-name:text-is("Closed port")', { timeout: 10000 });
const rows = await page.$$eval('.rows-checks .row', els => els.map(e => ({
  name: e.querySelector('.row-name')?.textContent,
  state: e.querySelector('.row-bar')?.dataset.state,
  detail: e.querySelector('.row-detail')?.textContent,
})));
console.log('host uptime   :', rows.map(r => `${r.name} ${r.state}`).join(', '));
const want = { 'GhostDock itself': 'running', 'Closed port': 'unhealthy' };
for (const [name, state] of Object.entries(want)) {
  const row = rows.find(r => r.name === name);
  if (row?.state !== state) problems.push(`${name} is ${row?.state}, not ${state}`);
}
await page.screenshot({ path: `${SHOTS}/checks-host.png`, fullPage: true });

// 5. A stack's own check, down, marks its row on the board and is listed
// on its page.
const made = await page.request.post(`${BASE}/api/v1/hosts/1/stacks`, {
  data: { name: 'Uptime demo', compose_yaml: 'services: {}\n' },
});
const stack = await made.json();
const linked = await page.request.post(`${BASE}/api/v1/hosts/1/checks`, {
  data: { name: 'Demo port', kind: 'tcp', target: closed, retries: 1, stack_id: stack.id },
});
if (!linked.ok()) problems.push(`could not link a check: ${linked.status()}`);
await page.click('.nav-item:has-text("Stacks")');
await page.waitForSelector(`.row:has(.row-name:text-is("${stack.slug}")) .row-alert:has-text("down: Demo port")`, { timeout: 20000 })
  .catch(() => problems.push('the board does not mark the stack whose check is down'));
const markState = await page.getAttribute(`.row:has(.row-name:text-is("${stack.slug}")) .row-alert .state-mark`, 'data-state').catch(() => null);
if (markState !== 'unhealthy') problems.push(`the board mark is ${markState}`);
await page.screenshot({ path: `${SHOTS}/checks-board.png`, fullPage: true });
await page.click(`.row-link:has(.row-name:text-is("${stack.slug}"))`);
await page.waitForSelector('.rows-checks .row-name:text-is("Demo port")', { timeout: 10000 })
  .catch(() => problems.push('the stack page does not list its check'));
console.log('stack         :', `${stack.slug} marked on the board and lists Demo port`);

const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (sideways > 0) problems.push(`scrolls sideways by ${sideways}px`);

await browser.close();
hook.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nUptime checks and alerts work end to end.');
process.exit(problems.length ? 1 : 0);
