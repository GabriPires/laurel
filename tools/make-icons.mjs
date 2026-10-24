#!/usr/bin/env node
// Renders Laurel's icons (wiki/projeto/guia-de-estilo.md#ícone) with headless Edge on
// the Windows side, through the DevTools protocol, so every size is drawn at its own
// pixel size: the app icon (app/icons/icon.ico from 16 to 256 px, and icon.png), the
// tray icons for each recorder state, taskbar theme and size (app/icons/tray/), and
// app/src/tray_icons.rs, which embeds the tray icons.
// Usage (from WSL, with the mirrored network): node tools/make-icons.mjs
import { execSync, spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const WINUSER = process.env.WINUSER ?? execSync('cmd.exe /c echo %USERNAME%', { cwd: '/mnt/c', encoding: 'utf8' }).trim();
const EDGE = '/mnt/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const PROFILE = `C:\\Users\\${WINUSER}\\laurel-dev\\icons-profile`;
const PORT = 9224;

// The branch of the title bar logo (24 px grid), leaves filled. Its box with the
// stroke is x 6.65..21.45 and y 4.05..21.95, centered on (14.05, 13).
function branch(color, scale, stroke = 1.3, stem = 1.9) {
  const tx = 32 - 14.05 * scale;
  const ty = 32 - 13 * scale;
  return `<g transform="translate(${tx.toFixed(2)} ${ty.toFixed(2)}) scale(${scale})" fill="${color}" stroke="${color}" stroke-width="${stroke}" stroke-linecap="round" stroke-linejoin="round">`
    + `<path d="M12 21c0-6 2-11 7-15" fill="none" stroke-width="${stem}"/>`
    + '<path d="M14.5 13.5c2.5.3 4.5-.6 5.5-2.5-2.4-.4-4.4.4-5.5 2.5z"/>'
    + '<path d="M13 17c-2.6.2-4.6-.9-5.4-2.9 2.4-.3 4.4.7 5.4 2.9z"/>'
    + '<path d="M16.5 9.5c.4-2.4 1.9-4 4-4.5-.1 2.3-1.5 3.9-4 4.5z"/>'
    + '<path d="M12.6 11.8c-2.3-.7-3.6-2.5-3.6-4.6 2.2.5 3.6 2.2 3.6 4.6z"/></g>';
}

// App icon: the jade branch on the dark tile.
const appIcon = '<rect x="2" y="2" width="60" height="60" rx="14" fill="#0B0C12" stroke="#2A2E3D" stroke-width="1.5"/>' + branch('#12D99B', 2.46);
// Tray icon: no tile, the whole branch in the state color; a darker set for the
// light taskbar keeps every state above 3:1 against it.
const TRAY_COLORS = {
  dark: { recording: '#12D99B', waiting: '#969BAF', off: '#FFC547', attention: '#FF5470' },
  light: { recording: '#0B8F66', waiting: '#6B7085', off: '#A87800', attention: '#D6334F' },
};
const trayIcon = (color) => branch(color, 3.13, 1.5, 2.1);
const APP_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const TRAY_SIZES = [16, 20, 24, 32, 40, 48];

// Every icon gets a slot on one page; each is then captured by its own clip.
const shots = [];
let x = 0;
let y = 0;
let rowHeight = 0;
function place(name, size, inner) {
  if (x + size > 1500) {
    x = 0;
    y += rowHeight + 8;
    rowHeight = 0;
  }
  shots.push({ name, size, x, y, inner });
  x += size + 8;
  rowHeight = Math.max(rowHeight, size);
}
place('icon-512', 512, appIcon);
for (const size of APP_SIZES) place(`icon-${size}`, size, appIcon);
for (const [theme, colors] of Object.entries(TRAY_COLORS)) {
  for (const [state, color] of Object.entries(colors)) {
    for (const size of TRAY_SIZES) place(`tray/${theme}-${state}-${size}`, size, trayIcon(color));
  }
}
const page = '<!doctype html><html><body style="margin:0;background:transparent">'
  + shots.map((s) => `<svg style="position:absolute;left:${s.x}px;top:${s.y}px" width="${s.size}" height="${s.size}" viewBox="0 0 64 64">${s.inner}</svg>`).join('')
  + '</body></html>';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function devtoolsPage() {
  for (let i = 0; i < 60; i++) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const target = targets.find((t) => t.type === 'page');
      if (target) return target.webSocketDebuggerUrl;
    } catch {
      // Edge is still starting.
    }
    await sleep(500);
  }
  throw new Error('headless Edge did not open its DevTools port');
}

function client(url) {
  const sock = new WebSocket(url);
  let id = 0;
  const pending = new Map();
  sock.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.error) p.reject(new Error(msg.error.message));
    else p.resolve(msg.result);
  };
  const ready = new Promise((resolve, reject) => {
    sock.onopen = resolve;
    sock.onerror = reject;
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const n = ++id;
    pending.set(n, { resolve, reject });
    sock.send(JSON.stringify({ id: n, method, params }));
  });
  return { ready, send, close: () => sock.close() };
}

// An .ico whose entries are PNG images (Windows Vista and later).
function ico(images) {
  const header = Buffer.alloc(6 + 16 * images.length);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = header.length;
  images.forEach(({ size, png }, i) => {
    const at = 6 + 16 * i;
    header.writeUInt8(size >= 256 ? 0 : size, at);
    header.writeUInt8(size >= 256 ? 0 : size, at + 1);
    header.writeUInt16LE(1, at + 4);
    header.writeUInt16LE(32, at + 6);
    header.writeUInt32LE(png.length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([header, ...images.map((i) => i.png)]);
}

const edge = spawn(EDGE, ['--headless=new', '--disable-gpu', `--remote-debugging-port=${PORT}`, `--user-data-dir=${PROFILE}`, 'about:blank'], { stdio: 'ignore' });
let cdp;
try {
  cdp = client(await devtoolsPage());
  await cdp.ready;
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1400, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 0, g: 0, b: 0, a: 0 } });
  await cdp.send('Page.navigate', { url: `data:text/html;base64,${Buffer.from(page).toString('base64')}` });
  await sleep(1000);
  const pngs = new Map();
  mkdirSync(join(ROOT, 'app/icons/tray'), { recursive: true });
  for (const s of shots) {
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', clip: { x: s.x, y: s.y, width: s.size, height: s.size, scale: 1 } });
    const png = Buffer.from(data, 'base64');
    pngs.set(s.name, png);
    if (s.name.startsWith('tray/')) writeFileSync(join(ROOT, 'app/icons', `${s.name}.png`), png);
  }
  writeFileSync(join(ROOT, 'app/icons/icon.png'), pngs.get('icon-512'));
  writeFileSync(join(ROOT, 'app/icons/icon.ico'), ico(APP_SIZES.map((size) => ({ size, png: pngs.get(`icon-${size}`) }))));

  const arms = [];
  for (const theme of Object.keys(TRAY_COLORS)) {
    for (const state of Object.keys(TRAY_COLORS[theme])) {
      for (const size of TRAY_SIZES) arms.push(`        ("${theme}", "${state}", ${size}) => include_bytes!("../icons/tray/${theme}-${state}-${size}.png"),`);
    }
  }
  writeFileSync(join(ROOT, 'app/src/tray_icons.rs'), `//! Tray icons by taskbar theme, recorder state and size, generated by
//! tools/make-icons.mjs (do not edit by hand).

/// The sizes drawn, in pixels.
pub const SIZES: [u32; ${TRAY_SIZES.length}] = [${TRAY_SIZES.join(', ')}];

/// The PNG for a taskbar theme ("dark" or "light"), a recorder state ("recording",
/// "waiting", "off" or "attention") and one of \`SIZES\`.
pub fn png(theme: &str, state: &str, size: u32) -> Option<&'static [u8]> {
    Some(match (theme, state, size) {
${arms.join('\n')}
        _ => return None,
    })
}
`);
  console.log(`[icons] app icon: ${APP_SIZES.join(', ')} px; tray: ${arms.length} icons`);
  await cdp.send('Browser.close').catch(() => {});
} finally {
  cdp?.close();
  edge.kill();
}
