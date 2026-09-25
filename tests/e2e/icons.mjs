// Stack icons: a stack running nginx shows nginx's mark, drawn in the ink in
// both themes, on the board and its own page; a stack running nothing known
// shows its initials. Icons are decorative, so neither adds to any name.
//
// Needs a live server and a Docker daemon. Registers, deploys, takes down
// and forgets its own two stacks, so it needs no fixture. See run.sh.
import { AxeBuilder } from '@axe-core/playwright';
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const PASSWORD = 'correct horse battery staple';
const WEB = 'ghostdock-e2e-icons-web';
const PLAIN = 'ghostdock-e2e-icons-plain';
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];
const FIXTURES = [
  [WEB, 'services:\n  web:\n    image: nginx:alpine\n'],
  [PLAIN, 'services:\n  box:\n    image: alpine:3.22\n    command: ["sh", "-c", "while :; do sleep 3600; done"]\n'],
];

const browser = await chromium.launch();
const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
const page = await ctx.newPage();
const problems = [];
page.on('console', m => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
const fetched = [];
page.on('response', r => {
  if (r.url().includes('/brand-icons/')) fetched.push([new URL(r.url()).pathname, r.status()]);
});

// Icons must not cost the screen its WCAG AA standing.
const audit = async label => {
  const { violations } = await new AxeBuilder({ page }).withTags(TAGS).analyze();
  for (const v of violations) {
    const where = v.nodes.slice(0, 3).map(n => n.target.join(' ')).join(' | ');
    problems.push(`${label}: ${v.id} (${v.impact}) ${v.nodes.length}x: ${where}`);
  }
  console.log(`axe, ${label}`.padEnd(14) + ':', violations.length ? `${violations.length} violation(s)` : 'clean');
};

// Through the API, with the browser's own session.
const api = async (method, path, data) => {
  const r = await ctx.request.fetch(`${BASE}/api/v1${path}`, { method, data });
  const body = await r.text();
  if (!r.ok()) throw new Error(`${method} ${path}: ${r.status()} ${body}`);
  return body ? JSON.parse(body) : null;
};
const operate = async (id, verb) => {
  const started = await api('POST', `/stacks/${id}/${verb}`);
  for (let i = 0; i < 180; i++) {
    const d = await api('GET', `/deployments/${started.id}`);
    if (d.status !== 'running') return d;
    await new Promise(r => setTimeout(r, 1000));
  }
  throw new Error(`${verb} did not finish`);
};

// A fresh server is set up, and signed in to, by its first account; a used
// one is signed in to.
const boot = await ctx.request.post(`${BASE}/api/v1/auth/bootstrap`, {
  data: { username: 'admin', password: PASSWORD },
});
await page.goto(BASE, { waitUntil: 'networkidle' });
if (!boot.ok()) {
  await page.fill('input[type=text]', 'admin');
  await page.fill('input[type=password]', PASSWORD);
  await page.click('button[type=submit]');
}
await page.waitForSelector('.nav-item', { timeout: 30000 });

const ids = [];
try {
  for (const [name, yaml] of FIXTURES) {
    const stack = await api('POST', '/hosts/1/stacks', { name, compose_yaml: yaml });
    ids.push(stack.id);
    const done = await operate(stack.id, 'deploy');
    if (done.status !== 'succeeded') throw new Error(`could not deploy ${name}: ${done.log}`);
  }

  const brandOf = project => `.row-link:has(.row-name:text-is("${project}")) .brand`;
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForSelector(`${brandOf(WEB)}[data-icon="nginx"]`, { timeout: 20000 });
  await page.waitForSelector(`${brandOf(PLAIN)}[data-letters]`, { timeout: 20000 });

  // Drawn in the ink: the mark is a mask over the text color, whatever the
  // theme, and never a brand's own color.
  const look = async () => {
    const mark = await page.locator(brandOf(WEB)).evaluate(e => {
      const s = getComputedStyle(e);
      return {
        ink: getComputedStyle(document.body).color,
        fill: s.backgroundColor,
        mask: s.maskImage || s.webkitMaskImage,
        hidden: e.getAttribute('aria-hidden'),
        name: e.closest('.row-name').textContent,
      };
    });
    const tile = await page.locator(brandOf(PLAIN)).evaluate(e => ({
      letters: e.dataset.letters,
      icon: e.dataset.icon ?? null,
      shown: getComputedStyle(e, '::before').content,
      hidden: e.getAttribute('aria-hidden'),
      name: e.closest('.row-name').textContent,
    }));
    return {
      ...mark,
      letters: tile.letters,
      tileIcon: tile.icon,
      shown: tile.shown,
      hidden: [mark.hidden, tile.hidden],
      names: [mark.name, tile.name],
    };
  };

  // Fetched only once a row shows it.
  for (let i = 0; i < 50 && !fetched.some(([path]) => path === '/brand-icons/nginx.svg'); i++) {
    await page.waitForTimeout(100);
  }
  const dark = await look();
  console.log('board, dark   :', `nginx mark ${dark.fill} on ink ${dark.ink}, other shows ${dark.shown}`);
  if (!dark.mask.includes('/brand-icons/nginx.svg')) problems.push(`the mark is not nginx's: ${dark.mask}`);
  if (dark.fill !== dark.ink) problems.push(`the mark is ${dark.fill}, not the ink ${dark.ink}`);
  if (dark.letters !== 'GE' || dark.shown !== '"GE"') problems.push(`initials are ${dark.letters} (${dark.shown})`);
  if (dark.tileIcon !== null) problems.push('an unknown stack claims an icon');
  if (!dark.hidden.every(h => h === 'true')) problems.push('an icon is not hidden from assistive technology');
  if (dark.names[0] !== WEB || dark.names[1] !== PLAIN) problems.push(`an icon changed a name: ${dark.names}`);
  if (!fetched.some(([path, status]) => path === '/brand-icons/nginx.svg' && status < 400)) {
    problems.push(`nginx.svg was not served: ${JSON.stringify(fetched)}`);
  }
  await audit('board, dark');
  await page.screenshot({ path: `${SHOTS}/icons-board-dark.png`, fullPage: true });

  await page.emulateMedia({ colorScheme: 'light' });
  const light = await look();
  console.log('board, light  :', `nginx mark ${light.fill} on ink ${light.ink}`);
  if (light.fill !== light.ink || light.ink === dark.ink) problems.push(`light theme mark is ${light.fill}, ink ${light.ink}`);
  await audit('board, light');
  await page.screenshot({ path: `${SHOTS}/icons-board-light.png`, fullPage: true });
  await page.emulateMedia({ colorScheme: 'dark' });

  // The stack's own page shows the same mark in its heading, which still
  // reads as the stack's name alone.
  await page.click(`.row-link:has(.row-name:text-is("${WEB}"))`);
  await page.waitForSelector('h1.wordmark .brand[data-icon="nginx"]', { timeout: 20000 });
  const heading = await page.textContent('h1.wordmark');
  console.log('stack page    :', `heading "${heading}" with its mark`);
  if (heading !== WEB) problems.push(`the heading reads "${heading}"`);
  await audit('stack page');
  await page.screenshot({ path: `${SHOTS}/icons-stack.png`, fullPage: true });

  await page.goBack();
  await page.click(`.row-link:has(.row-name:text-is("${PLAIN}"))`);
  await page.waitForSelector('h1.wordmark .brand[data-letters="GE"]', { timeout: 20000 });
  console.log('other page    :', 'heading shows its initials');
} catch (e) {
  problems.push(String(e.message ?? e));
} finally {
  // Only what this test made.
  for (const id of ids) {
    try {
      await operate(id, 'down');
      await api('DELETE', `/stacks/${id}`);
    } catch (e) {
      problems.push(`cleanup: ${e.message}`);
    }
  }
}

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nStacks show what they run, in the ink.');
process.exit(problems.length ? 1 : 0);
