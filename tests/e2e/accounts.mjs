// Accounts, end to end in a real browser: add a second person, change your
// own password and watch another device get signed out, then remove the
// second account.
//
// Needs a live server with a FRESH database. See tests/e2e/run.sh.
import { chromium, devices } from 'playwright';

const BASE = process.env.GHOSTDOCK_URL ?? 'http://127.0.0.1:18081';
const SHOTS = process.env.SHOTS ?? 'target/e2e-shots';
const PASSWORD = 'correct horse battery staple';
const NEW_PASSWORD = 'an entirely different phrase';

const browser = await chromium.launch();
const problems = [];

async function device() {
  const ctx = await browser.newContext({ ...devices['iPhone 14'], colorScheme: 'dark' });
  const page = await ctx.newPage();
  page.on('console', m => {
    // Deliberate failures below (a wrong password, a 401 after sign-out) log
    // resource errors; the assertions check those outcomes directly.
    if (m.type() === 'error' && !m.text().includes('Failed to load resource')) {
      problems.push(`console: ${m.text()}`);
    }
  });
  page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
  return page;
}

async function signIn(page, user, password) {
  await page.goto(BASE, { waitUntil: 'networkidle' });
  await page.waitForSelector('h1.entry-heading');
  await page.fill('input[type=text]', user);
  await page.fill('input[type=password]', password);
  await page.click('button[type=submit]');
  await page.waitForSelector('.nav-item', { timeout: 30000 });
}

const laptop = await device();
await signIn(laptop, 'admin', PASSWORD);

const phone = await device();
await signIn(phone, 'admin', PASSWORD);

// 1. Add a second account from Settings.
await laptop.click('.nav-item:has-text("Settings")');
await laptop.click('a:has-text("Accounts and password")');
await laptop.waitForSelector('h1.wordmark:has-text("Accounts")');
const add = laptop.locator('form').filter({ has: laptop.locator('button:has-text("Add account")') });
await add.locator('input[type=text]').fill('second');
await add.locator('input[type=password]').fill('a long enough password');
await add.locator('button[type=submit]').click();
await laptop.waitForSelector('.row-name:text("second")');
const names = await laptop.$$eval('.rows .row-name', els => els.map(e => e.textContent));
console.log('accounts      :', names.join(', '));
await laptop.screenshot({ path: `${SHOTS}/accounts.png`, fullPage: true });

// 2. A wrong current password is refused with a reason.
const pw = laptop.locator('form').filter({ has: laptop.locator('button:has-text("Change password")') });
await pw.locator('input[autocomplete=current-password]').fill('not my password');
await pw.locator('input[autocomplete=new-password]').fill(NEW_PASSWORD);
await pw.locator('button[type=submit]').click();
const refusal = await pw.locator('.notice').textContent({ timeout: 10000 });
console.log('wrong current :', refusal);

// 3. The right one changes it, keeps this device signed in, and signs the
// phone out.
await pw.locator('input[autocomplete=current-password]').fill(PASSWORD);
await pw.locator('input[autocomplete=new-password]').fill(NEW_PASSWORD);
await pw.locator('button[type=submit]').click();
await laptop.waitForSelector('text=Every other device signed in as you has been signed out.');
await laptop.reload({ waitUntil: 'networkidle' });
if (!(await laptop.$('h1.wordmark:has-text("Accounts")'))) {
  problems.push('changing the password signed out the device that changed it');
}
await phone.reload({ waitUntil: 'networkidle' });
await phone.waitForSelector('h1.entry-heading', { timeout: 10000 });
console.log('phone         :', (await phone.textContent('h1.entry-heading')).trim());

await signIn(phone, 'admin', NEW_PASSWORD);
console.log('new password  : signs in');

// 4. Removing takes two taps; your own row has no remove at all.
const own = laptop.locator('.row', { has: laptop.locator('.row-name:text("admin")') });
if (await own.locator('.row-action').count()) problems.push('your own account offers a remove button');
const second = laptop.locator('.row', { has: laptop.locator('.row-name:text("second")') });
await second.locator('.row-action').click();
await laptop.waitForTimeout(300);
if (!(await laptop.$('.row-name:text("second")'))) problems.push('one tap removed an account');
await second.locator('.row-action:has-text("Confirm")').click();
await laptop.waitForSelector('.row-name:text("second")', { state: 'detached', timeout: 10000 });
console.log('removed       : second');

const overflow = await laptop.evaluate(() =>
  document.documentElement.scrollWidth - document.documentElement.clientWidth);
if (overflow > 0) problems.push(`page scrolls horizontally by ${overflow}px`);

await browser.close();
console.log(problems.length ? `\nPROBLEMS:\n- ${problems.join('\n- ')}` : '\nNo console errors, no overflow.');
process.exit(problems.length ? 1 : 0);
