// Matty web benchmark (docs/web.md). Needs playwright or playwright-core. Usage:
//   node scripts/web-bench.mjs <label> <dist-dir | https-url> [--dpr 2] [--backends auto,webgl] [--channel chrome|chromium] [--headed]
// Serves a local dist with COOP/COEP, opens the demo workspace, and measures
// startup, scrolling the Standup channel, hovering messages and scrolling the
// sidebar. Prints one JSON line per scenario and appends them to results.jsonl.
import { createServer } from 'node:http';
import { readFile, appendFile, stat } from 'node:fs/promises';
import { extname, join, resolve } from 'node:path';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
let playwright;
try { playwright = require('playwright'); } catch { playwright = require('playwright-core'); }

const args = process.argv.slice(2);
const label = args[0]; const target = args[1];
const opt = (k, d) => { const i = args.indexOf('--' + k); return i >= 0 ? args[i + 1] : d; };
const dpr = Number(opt('dpr', '0')); // 0: no emulation, the display's own pixel ratio (headed)
const trace = args.includes('--trace');
const backends = opt('backends', 'auto,webgl').split(',');
const channel = opt('channel', 'chrome');
const headed = args.includes('--headed');

let base = target;
let server;
if (!/^https?:/.test(target)) {
  const root = resolve(target);
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.svg': 'image/svg+xml', '.ttf': 'font/ttf' };
  server = createServer(async (req, res) => {
    let p = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    if (p.endsWith('/')) p += 'index.html';
    try {
      const body = await readFile(join(root, p));
      res.writeHead(200, { 'Content-Type': types[extname(p)] || 'application/octet-stream', 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp', 'Cache-Control': 'no-store' });
      res.end(body);
    } catch { res.writeHead(404); res.end(); }
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  base = `http://127.0.0.1:${server.address().port}/`;
}

const launchOpts = { headless: !headed, args: ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--window-size=1280,860'] };
if (channel !== 'chromium') launchOpts.channel = channel;
const browser = await playwright.chromium.launch(launchOpts);
const out = [];
const log = (o) => { const line = JSON.stringify({ label, ...o }); console.log(line); out.push(line); };

for (const backend of backends) {
  const ctx = await browser.newContext(dpr ? { viewport: { width: 1280, height: 800 }, deviceScaleFactor: dpr } : { viewport: null });
  const page = await ctx.newPage();
  const consoleLines = [];
  page.on('console', m => consoleLines.push(m.text().slice(0, 200)));
  await page.addInitScript(() => {
    window.__frames = [];
    const raf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = (cb) => raf((ts) => { const s = performance.now(); cb(ts); window.__frames.push([s, performance.now() - s]); });
  });
  const url = base + (backend === 'webgl' ? '?backend=webgl' : '');
  const t0 = Date.now();
  await page.goto(url);
  await page.waitForFunction(() => document.getElementById('loading')?.hidden, null, { timeout: 180000 });
  const ready = Date.now() - t0;
  await page.waitForTimeout(3000);
  const env = await page.evaluate(async () => {
    const nav = performance.getEntriesByType('resource').filter(e => e.name.endsWith('.wasm')).map(e => ({ ms: Math.round(e.duration), bytes: e.transferSize, decoded: e.decodedBodySize }));
    let gpu = 'none';
    if (navigator.gpu) { const a = await navigator.gpu.requestAdapter(); gpu = a ? JSON.stringify(a.info ? { vendor: a.info.vendor, arch: a.info.architecture } : {}) : 'no adapter'; }
    return { dpr: devicePixelRatio, ua: navigator.userAgent, wasm: nav, gpu, cores: navigator.hardwareConcurrency };
  });
  log({ backend, scenario: 'startup', readyMs: ready, ...env });

  const stats = () => page.evaluate(() => {
    const f = window.__frames; const el = performance.now() - window.__m0; const d = f.map(x => x[1]).sort((a, b) => a - b);
    const q = p => d.length ? +d[Math.min(d.length - 1, Math.floor(p * d.length))].toFixed(1) : 0;
    const gaps = f.slice(1).map((x, i) => x[0] - f[i][0]).sort((a, b) => a - b);
    const g = p => gaps.length ? +gaps[Math.min(gaps.length - 1, Math.floor(p * gaps.length))].toFixed(1) : 0;
    return { frames: f.length, elapsedMs: Math.round(el), fps: +(f.length / el * 1000).toFixed(1), cbP50: q(.5), cbP90: q(.9), cbMax: q(1), gapP50: g(.5), gapP90: g(.9), gapMax: g(1), mainBusyPct: Math.round(d.reduce((a, b) => a + b, 0) / el * 100) };
  });
  const real = async (name, fn) => {
    if (trace) await browser.startTracing(page, { path: `trace-${label}-${backend}-${name}.json`, categories: ['devtools.timeline', 'gpu', 'viz', 'toplevel', 'blink'] });
    await page.evaluate(() => { window.__frames = []; window.__m0 = performance.now(); });
    await fn();
    await page.waitForTimeout(300);
    const r = await stats();
    if (trace) await browser.stopTracing();
    log({ backend, scenario: name, input: 'real', ...r });
  };
  const run = (name, dy, x, y, n) => page.evaluate(([dy, x, y, n]) => new Promise(res => {
    window.__frames = []; const c = document.querySelector('canvas'); const t0 = performance.now(); let i = 0;
    const tick = () => {
      if (dy === 0) c.dispatchEvent(new PointerEvent('pointermove', { clientX: x + (i % 2) * 6, clientY: y + (i * 7) % 420, bubbles: true, pointerType: 'mouse', isPrimary: true }));
      else c.dispatchEvent(new WheelEvent('wheel', { deltaY: dy, deltaMode: 0, clientX: x, clientY: y, bubbles: true, cancelable: true }));
      if (++i < n) setTimeout(tick, 16); else setTimeout(() => {
        const f = window.__frames; const el = performance.now() - t0; const d = f.map(x => x[1]).sort((a, b) => a - b);
        const q = p => d.length ? +d[Math.min(d.length - 1, Math.floor(p * d.length))].toFixed(1) : 0;
        res({ frames: f.length, elapsedMs: Math.round(el), fps: +(f.length / el * 1000).toFixed(1), p50: q(.5), p90: q(.9), max: q(1), mainBusyPct: Math.round(d.reduce((a, b) => a + b, 0) / el * 100) });
      }, 300);
    };
    tick();
  }), [dy, x, y, n]).then(r => log({ backend, scenario: name, ...r }));

  await page.screenshot({ path: `shot-${label}-${backend}-start.png` });
  await page.mouse.click(137, 589); // Standup in the demo sidebar
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `shot-${label}-${backend}-standup.png` });
  await page.mouse.move(800, 400);
  await real('wheel-messages-up', async () => { for (let i = 0; i < 60; i++) { await page.mouse.wheel(0, -60); await page.waitForTimeout(8); } });
  await real('wheel-messages-down', async () => { for (let i = 0; i < 60; i++) { await page.mouse.wheel(0, 60); await page.waitForTimeout(8); } });
  await real('mouse-hover', async () => { for (let i = 0; i < 60; i++) { await page.mouse.move(700 + (i % 2) * 6, 150 + (i * 7) % 420); await page.waitForTimeout(16); } });
  await page.mouse.click(700, 760); // the composer
  await real('typing', async () => { await page.keyboard.type('The quick brown fox jumps over the lazy dog, twice over.', { delay: 30 }); });
  await page.mouse.move(150, 400);
  await real('wheel-sidebar', async () => { for (let i = 0; i < 30; i++) { await page.mouse.wheel(0, 40); await page.waitForTimeout(8); } });
  await run('scroll-messages-up', -60, 800, 400, 120);
  await run('scroll-messages-down', 60, 800, 400, 120);
  await run('hover-messages', 0, 700, 150, 90);
  await run('scroll-sidebar', 40, 150, 400, 60);
  await page.screenshot({ path: `shot-${label}-${backend}.png` });
  log({ backend, scenario: 'console-tail', lines: consoleLines.slice(-8) });
  await ctx.close();
}
await browser.close();
server?.close();
await appendFile('results.jsonl', out.join('\n') + '\n');
