#!/usr/bin/env node
// Check the website and its demo in a headless browser, and take the release review's
// screenshots: desktop and phone width, light and dark.
//
// It checks what the spec (docs/DESIGN.md, "Website") promises and a unit test cannot see: the
// demo lays out 104x26 on a desktop and narrows on a phone, the page never scrolls sideways,
// "Try the demo" gives the demo the keyboard and esc / Tab give it back, keys reach the real
// renderer, and with the wasm blocked the page says so and still works.
//
// Needs the demo built (scripts/build-demo.sh) and Playwright with its Chromium:
//
//   npm i --no-save playwright@1.58 && npx playwright install chromium
//   node scripts/site-check.cjs [out-dir]          # default: site-check/ (gitignored)
//
// Not in CI: it downloads a browser on every run, and the demo's behaviour is already held to
// the terminal's by `ui::parity` in `cargo test`. Run it for a release or a change to the site.
'use strict';
const http = require('http');
const fs = require('fs');
const path = require('path');

let chromium;
try {
  ({ chromium } = require('playwright'));
} catch {
  console.error('site-check: playwright not found — npm i --no-save playwright@1.58 && npx playwright install chromium');
  process.exit(2);
}

const root = path.join(__dirname, '..', 'docs');
const out = path.resolve(process.argv[2] || 'site-check');
if (!fs.existsSync(path.join(root, 'demo', 'sessio_bg.wasm'))) {
  console.error('site-check: docs/demo is not built — run scripts/build-demo.sh');
  process.exit(2);
}
fs.mkdirSync(out, { recursive: true });

const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' };
const server = http.createServer((req, res) => {
  const p = path.join(root, decodeURIComponent(req.url.split('?')[0]).replace(/\/$/, '/index.html'));
  if (!p.startsWith(root) || !fs.existsSync(p)) { res.writeHead(404); return res.end(); }
  res.writeHead(200, { 'content-type': TYPES[path.extname(p)] || 'application/octet-stream' });
  fs.createReadStream(p).pipe(res);
});

const failures = [];
const check = (ok, label) => { console.log((ok ? '  ok   ' : '  FAIL ') + label); if (!ok) failures.push(label); };

(async () => {
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const url = `http://127.0.0.1:${server.address().port}/`;
  const browser = await chromium.launch();
  const views = { desktop: { width: 1280, height: 900 }, mobile: { width: 390, height: 844 } };

  for (const [view, viewport] of Object.entries(views)) {
    for (const scheme of ['light', 'dark']) {
      console.log(`[${view} · ${scheme}]`);
      const page = await browser.newPage({ viewport, colorScheme: scheme, isMobile: view === 'mobile', hasTouch: view === 'mobile' });
      await page.goto(url);
      await page.waitForFunction(() => document.getElementById('screen').textContent.includes('everything'), null, { timeout: 15000 });
      const size = await page.textContent('#term-size');
      check(view === 'desktop' ? size.includes('104×26') : /\b60×18\b/.test(size), `the demo lays out for the width (${size})`);
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      check(sideways <= 0, `the page does not scroll sideways (${sideways}px over)`);
      await page.screenshot({ path: path.join(out, `${view}-${scheme}.png`), fullPage: true });

      if (scheme === 'dark') {
        await page.click('#demo-toggle');
        const focused = await page.evaluate(() => document.activeElement && document.activeElement.id);
        check(focused === 'screen', `Try the demo gives it the keyboard (focus: ${focused})`);
        check((await page.textContent('#demo-state')).includes('keys go to the demo'), 'and says so');
        const before = await page.textContent('#screen');
        await page.keyboard.press('ArrowDown');
        await page.keyboard.press('ArrowRight');
        check((await page.textContent('#screen')) !== before, 'keys reach the renderer');
        await page.keyboard.press('?');
        check((await page.textContent('#screen')).includes('keys and marks'), '? opens the help overlay');
        await page.screenshot({ path: path.join(out, `${view}-demo-help.png`) });
        await page.keyboard.press('x');
        check(!(await page.textContent('#screen')).includes('keys and marks'), 'any key closes it');
        await page.keyboard.press('Escape');
        const after = await page.evaluate(() => document.activeElement && document.activeElement.id);
        check(after !== 'screen', `esc leaves the demo (focus: ${after})`);
        await page.click('#demo-toggle');
        await page.keyboard.press('Tab');
        const tabbed = await page.evaluate(() => document.activeElement && document.activeElement.id);
        check(tabbed !== 'screen', `Tab moves focus on (focus: ${tabbed})`);
      }
      await page.close();
    }
  }

  console.log('[wasm blocked]');
  const page = await browser.newPage({ viewport: views.desktop });
  await page.route('**/demo/**', (r) => r.abort());
  await page.goto(url);
  await page.waitForFunction(() => document.getElementById('demo-state').textContent.includes('unavailable'), null, { timeout: 15000 });
  check((await page.textContent('#screen')).includes("couldn't load"), 'the screen says the demo could not load');
  check(await page.isVisible('#install-cmd'), 'the install command is still there');
  await page.screenshot({ path: path.join(out, 'desktop-wasm-blocked.png') });

  await browser.close();
  server.close();
  console.log(failures.length ? `\nSITE CHECK: ${failures.length} FAILED` : `\nSITE CHECK: ok · screenshots in ${out}`);
  process.exit(failures.length ? 1 : 0);
})().catch((e) => { console.error(e); server.close(); process.exit(1); });
