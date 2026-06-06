import assert from 'node:assert/strict';
import http from 'node:http';
import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from 'playwright';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');

const mimeTypes = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.css', 'text/css; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
]);

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    const requestedPath = url.pathname === '/' ? '/index.html' : url.pathname;
    const filePath = path.join(root, requestedPath);
    if (!filePath.startsWith(root))
      throw new Error('Invalid path');

    const body = await readFile(filePath);
    response.writeHead(200, {'content-type': mimeTypes.get(path.extname(filePath)) ?? 'application/octet-stream'});
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end('Not found');
  }
});

await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const {port} = server.address();

const browser = await chromium.launch({
  executablePath: process.env.CHROME_BIN ?? '/usr/bin/google-chrome',
  headless: true,
  args: ['--no-sandbox'],
});

try {
  const page = await browser.newPage({viewport: {width: 1280, height: 800}});
  await page.goto(`http://127.0.0.1:${port}/`);

  await page.click('[data-testid="open"]');
  await page.waitForSelector('[data-testid="overlay"].is-open');

  const searchBox = await page.locator('[data-testid="search"]').boundingBox();
  const taskbarBox = await page.locator('[data-testid="taskbar"]').boundingBox();
  const scrollBox = await page.locator('[data-testid="shelf-scroll"]').boundingBox();
  assert(searchBox, 'search input has a bounding box');
  assert(taskbarBox, 'right taskbar has a bounding box');
  assert(scrollBox, 'shelf scroll has a bounding box');
  assert.equal(Math.round(searchBox.height), 34, 'search input is compact');
  assert(taskbarBox.x > scrollBox.x + scrollBox.width, 'taskbar is positioned to the right of the shelf');

  const cardCount = await page.locator('[data-testid="card"]').count();
  assert.equal(cardCount, 12, 'renders full mock history');

  const firstBox = await page.locator('[data-testid="card"]').first().boundingBox();
  assert(firstBox, 'first card has a bounding box');
  assert.equal(Math.round(firstBox.width), 160, 'card width is square size');
  assert.equal(Math.round(firstBox.height), 160, 'card height is square size');

  const scrollMetrics = await page.$eval('[data-testid="shelf-scroll"]', element => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  assert(scrollMetrics.scrollWidth > scrollMetrics.clientWidth, 'shelf scrolls horizontally when cards overflow');

  const imageBackground = await page.locator('[data-card-id="image-hero"] [data-testid="image-preview"]')
    .evaluate(element => getComputedStyle(element).backgroundImage);
  assert(imageBackground.includes('data:image/svg+xml'), 'image card renders an inline preview, not a path string');

  await page.locator('[data-card-id="cmd-build"]').click();
  await page.waitForFunction(() => !window.pastebarHarness.state().isOpen);

  let state = await page.evaluate(() => window.pastebarHarness.state());
  assert.equal(state.copyCount, 1, 'click copies one item');
  assert.equal(state.beepCount, 1, 'click emits feedback beep');
  assert.equal(state.closeCount, 1, 'click closes pastebar');
  assert.equal(state.clipboard.kind, 'text', 'text card copies text clipboard');
  assert.equal(state.clipboard.text, 'cargo build --release && gnome-extensions enable pastazzo@turinglabs.org');
  assert.equal(state.firstItemId, 'cmd-build', 'clicked history item is touched to first position');

  await page.click('[data-testid="open"]');
  await page.waitForSelector('[data-testid="overlay"].is-open');
  const firstAfterTouch = await page.locator('[data-testid="card"]').first().getAttribute('data-card-id');
  assert.equal(firstAfterTouch, 'cmd-build', 'reopened shelf shows latest copied item first at left');

  await page.locator('[data-card-id="cmd-build"]').dblclick();
  await page.waitForFunction(() => !window.pastebarHarness.state().isOpen);
  state = await page.evaluate(() => window.pastebarHarness.state());
  assert.equal(state.copyCount, 2, 'double click still copies one item');
  assert.equal(state.beepCount, 2, 'double click emits one feedback beep');
  assert.equal(state.closeCount, 2, 'double click closes pastebar');
  assert.equal(state.pasteCount, 1, 'double click pastes into the focused target');
  assert.equal(state.targetText, 'cargo build --release && gnome-extensions enable pastazzo@turinglabs.org');
  assert.equal(state.firstItemId, 'cmd-build', 'double-clicked item remains first at left');

  await page.click('[data-testid="open"]');
  await page.waitForSelector('[data-testid="overlay"].is-open');
  await page.locator('[data-card-id="image-hero"]').click();
  await page.waitForFunction(() => !window.pastebarHarness.state().isOpen);
  state = await page.evaluate(() => window.pastebarHarness.state());
  assert.equal(state.clipboard.kind, 'image', 'image card copies image clipboard shape');
  assert.equal(state.clipboard.mime, 'image/png', 'image card preserves image MIME');
  assert.equal(state.beepCount, 3, 'image click emits feedback beep');
  assert.equal(state.firstItemId, 'image-hero', 'clicked image becomes first item');

  await page.click('[data-testid="open"]');
  await page.waitForSelector('[data-testid="overlay"].is-open');
  await page.click('[data-testid="clear-history"]');
  await page.waitForSelector('[data-testid="empty"]');
  state = await page.evaluate(() => window.pastebarHarness.state());
  assert.equal(state.clearCount, 1, 'clear button clears history');
  assert.equal(state.itemIds.length, 0, 'history is empty after clear');
  assert.equal(state.beepCount, 4, 'clear button emits feedback beep');
  assert.equal(state.isOpen, true, 'clear button keeps pastebar open');
} finally {
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
