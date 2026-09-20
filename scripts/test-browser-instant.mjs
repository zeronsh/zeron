import assert from 'node:assert/strict';

const url = process.env.ZERON_BROWSER_URL;
const session = process.env.ZERON_BROWSER_SESSION;
if (!url || !session) {
  throw new Error('Set ZERON_BROWSER_URL and ZERON_BROWSER_SESSION before running this test.');
}

const playwright = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const { chromium } = playwright.default ?? playwright;
const browser = await chromium.launch({
  headless: true,
  args: ['--no-sandbox', '--enable-unsafe-webgpu', '--use-angle=swiftshader'],
});

try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.addCookies([{
    name: '__Host-comet_session',
    value: session,
    url: new URL(url).origin,
    secure: true,
    httpOnly: true,
    sameSite: 'Lax',
  }]);

  const page = await context.newPage();
  const failures = [];
  page.on('pageerror', error => failures.push(`pageerror: ${error}`));
  page.on('console', message => {
    if (message.type() === 'error') failures.push(`console: ${message.text()}`);
  });

  await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await page.waitForTimeout(15_000);

  const instantPanic = failures.find(message =>
    /time not implemented on this platform|RefCell already borrowed/i.test(message),
  );
  assert.equal(instantPanic, undefined, instantPanic);
  assert.deepEqual(failures, [], `unexpected browser errors: ${failures.join('\n')}`);
} finally {
  await browser.close();
}
