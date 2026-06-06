import http from 'node:http';
import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from 'playwright';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');
const outPath = path.resolve(root, 'settings-harness.png');

const mimeTypes = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.css', 'text/css; charset=utf-8'],
]);

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    const requestedPath = url.pathname === '/' ? '/settings.html' : url.pathname;
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
  const page = await browser.newPage({viewport: {width: 960, height: 640}});
  await page.goto(`http://127.0.0.1:${port}/settings.html`);
  await page.screenshot({path: outPath});
  console.log(outPath);
} finally {
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
