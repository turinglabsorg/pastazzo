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

  const headerBox = await page.locator('[data-testid="header"]').boundingBox();
  const searchBox = await page.locator('[data-testid="search"]').boundingBox();
  const taskbarBox = await page.locator('[data-testid="taskbar"]').boundingBox();
  const scrollBox = await page.locator('[data-testid="shelf-scroll"]').boundingBox();
  assert(headerBox, 'header has a bounding box');
  assert(searchBox, 'search input has a bounding box');
  assert(taskbarBox, 'right taskbar has a bounding box');
  assert(scrollBox, 'shelf scroll has a bounding box');
  assert.equal(Math.round(searchBox.height), 34, 'search input is compact');
  assert(taskbarBox.x > searchBox.x + searchBox.width, 'taskbar is positioned to the right of the search input');
  assert(Math.abs(taskbarBox.y - headerBox.y) < 2, 'taskbar is in the search header row');
  assert(scrollBox.y > headerBox.y + headerBox.height, 'shelf sits below the search header');
  assert(Math.abs(scrollBox.x - headerBox.x) < 2, 'shelf starts at the same left edge as the header');
  assert(scrollBox.width > searchBox.width, 'shelf keeps full width instead of losing space to the toolbar');
  const taskbarStyles = await page.locator('[data-testid="taskbar"]').evaluate(element => {
    const style = getComputedStyle(element);
    return {
      backgroundColor: style.backgroundColor,
      borderWidth: style.borderTopWidth,
    };
  });
  assert.equal(taskbarStyles.backgroundColor, 'rgba(0, 0, 0, 0)', 'taskbar rail is visually transparent');
  assert.equal(taskbarStyles.borderWidth, '0px', 'taskbar rail has no visible border');

  const clearButtonBox = await page.locator('[data-testid="clear-history"]').boundingBox();
  assert(clearButtonBox, 'clear button has a bounding box');
  assert.equal(Math.round(clearButtonBox.width), 38, 'clear button is fixed-width');
  assert.equal(Math.round(clearButtonBox.height), 38, 'clear button is square');
  const clearButtonRadius = await page.locator('[data-testid="clear-history"]')
    .evaluate(element => getComputedStyle(element).borderRadius);
  assert(clearButtonRadius === '999px' || clearButtonRadius === '50%', 'clear button is round');

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

  const scrollBehavior = await page.locator('[data-testid="shelf-scroll"]')
    .evaluate(element => getComputedStyle(element).scrollBehavior);
  assert.equal(scrollBehavior, 'smooth', 'shelf uses smooth browser scrolling in the harness');
  const scrollbarWidth = await page.locator('[data-testid="shelf-scroll"]')
    .evaluate(element => getComputedStyle(element).scrollbarWidth);
  assert.equal(scrollbarWidth, 'none', 'native scrollbar is hidden');

  await page.$eval('[data-testid="shelf-scroll"]', element => {
    element.scrollLeft = 0;
    element.dispatchEvent(new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      deltaY: 4,
    }));
  });
  await page.waitForFunction(() => document.querySelector('[data-testid="shelf-scroll"]').scrollLeft > 0);

  const imageBackground = await page.locator('[data-card-id="image-hero"] [data-testid="image-preview"]')
    .evaluate(element => getComputedStyle(element).backgroundImage);
  assert(imageBackground.includes('data:image/svg+xml'), 'image card renders an inline preview, not a path string');

  await page.fill('[data-testid="search"]', 'image/png');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="card"]').length === 1);
  const singleCardBox = await page.locator('[data-testid="card"]').first().boundingBox();
  const singleScrollBox = await page.locator('[data-testid="shelf-scroll"]').boundingBox();
  assert(singleCardBox, 'single result card has a bounding box');
  assert(singleScrollBox, 'single result shelf has a bounding box');
  assert.equal(Math.round(singleCardBox.width), 160, 'single result card keeps square width');
  assert.equal(Math.round(singleCardBox.height), 160, 'single result card keeps square height');
  assert(Math.abs(singleCardBox.x - singleScrollBox.x) < 2, 'single result card stays pinned to the left');

  await page.fill('[data-testid="search"]', '');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="card"]').length === 12);

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
