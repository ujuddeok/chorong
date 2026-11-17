// Renders the app icons in app/icons/<set>/ from their SVG sources: macOS .icns, Windows .ico
// and the Linux hicolor PNGs. Developer tool only; the outputs are committed and no build step
// runs this. It needs Playwright's Chromium (preinstalled in cloud sessions):
//
//   NODE_PATH="$(npm root -g)" node app/icons/render-icons.js
//
// Each set has <set>.svg (the 1024 master, drawn on the macOS icon grid: an 824 px tile with
// 100 px of margin) and optionally <set>-small.svg, a simplified drawing used at 32 px and below.
// macOS keeps the grid margin; Windows and Linux crop most of it so the tile fills small sizes.
'use strict';
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');
const { chromium } = require('playwright');

const HERE = __dirname;
const SETS = ['lantern', 'etched-m'];
const CROP = '92 92 840 840'; // Windows and Linux: keep 8 px around the tile
const SMALL_MAX = 32; // sizes at or below this use <set>-small.svg when it exists

// [icns type, pixel size] — the PNG-carrying types macOS reads, 16 pt to 512 pt @2x.
const ICNS = [['icp4', 16], ['icp5', 32], ['ic11', 32], ['ic12', 64], ['ic07', 128],
  ['ic13', 256], ['ic08', 256], ['ic14', 512], ['ic09', 512], ['ic10', 1024]];
const ICO = [16, 24, 32, 48, 64, 128, 256];
const HICOLOR = [16, 22, 24, 32, 48, 64, 128, 256, 512];

async function render(page, svg, size, viewBox) {
  let s = svg.replace(/width="1024" height="1024"/, `width="${size}" height="${size}"`);
  if (viewBox) s = s.replace(/viewBox="[^"]*"/, `viewBox="${viewBox}"`);
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(`<html><body style="margin:0;background:transparent">${s}</body></html>`);
  const png = await page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
  return optimise(png);
}

// Lossless recompression when ImageMagick is available; the raw screenshot otherwise.
function optimise(png) {
  try {
    return execFileSync('convert', ['png:-', '-strip', '-define', 'png:compression-level=9', 'png32:-'],
      { input: png, maxBuffer: 64 << 20 });
  } catch {
    return png;
  }
}

function icns(entries) {
  const chunks = entries.map(([type, png]) => {
    const head = Buffer.alloc(8);
    head.write(type, 0, 'ascii');
    head.writeUInt32BE(png.length + 8, 4);
    return Buffer.concat([head, png]);
  });
  const head = Buffer.alloc(8);
  head.write('icns', 0, 'ascii');
  head.writeUInt32BE(8 + chunks.reduce((n, c) => n + c.length, 0), 4);
  return Buffer.concat([head, ...chunks]);
}

function ico(entries) {
  const head = Buffer.alloc(6 + 16 * entries.length);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2); // icon
  head.writeUInt16LE(entries.length, 4);
  let offset = head.length;
  entries.forEach(([size, png], i) => {
    const e = 6 + 16 * i;
    head.writeUInt8(size >= 256 ? 0 : size, e); // 0 means 256
    head.writeUInt8(size >= 256 ? 0 : size, e + 1);
    head.writeUInt16LE(1, e + 4); // planes
    head.writeUInt16LE(32, e + 6); // bits per pixel
    head.writeUInt32LE(png.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  return Buffer.concat([head, ...entries.map(([, png]) => png)]);
}

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  for (const set of SETS) {
    const dir = path.join(HERE, set);
    const full = fs.readFileSync(path.join(dir, `${set}.svg`), 'utf8');
    const smallPath = path.join(dir, `${set}-small.svg`);
    const small = fs.existsSync(smallPath) ? fs.readFileSync(smallPath, 'utf8') : full;
    const pick = (size) => (size <= SMALL_MAX ? small : full);

    const mac = [];
    for (const [type, size] of ICNS) mac.push([type, await render(page, pick(size), size)]);
    fs.mkdirSync(path.join(dir, 'macos'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'macos', 'Chorong.icns'), icns(mac));

    const win = [];
    for (const size of ICO) win.push([size, await render(page, pick(size), size, CROP)]);
    fs.mkdirSync(path.join(dir, 'windows'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'windows', 'chorong.ico'), ico(win));

    for (const size of HICOLOR) {
      const out = path.join(dir, 'linux', 'hicolor', `${size}x${size}`, 'apps');
      fs.mkdirSync(out, { recursive: true });
      fs.writeFileSync(path.join(out, 'chorong.png'), await render(page, pick(size), size, CROP));
    }
    console.log(`${set}: macos/Chorong.icns, windows/chorong.ico, linux/hicolor (${HICOLOR.length} sizes)`);
  }
  await browser.close();
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
