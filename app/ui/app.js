// Main window shell: the title bar (recorder state, shortcuts, the recording switch,
// window buttons), the icon rail between Library and Settings, the banners, and the
// Advanced settings page (engine state, pipeline stats, activity).
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
let engineInfo = null;

const ICONS = {
  clip: '<path d="M5 12l4 4 10-10"/>',
  shot: '<path d="M4 8h3l2-3h6l2 3h3v11H4z"/><circle cx="12" cy="13" r="3.5"/>',
  bad: '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9L2.6 17a2 2 0 001.7 3h15.4a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z"/>',
  info: '<circle cx="12" cy="12" r="8"/><path d="M12 11v5M12 8h.01"/>',
};

// kind: clip | shot | bad | info (wiki/projeto/guia-de-estilo.md: violet for saved clips)
function activity(text, kind = 'info') {
  const list = $('activity');
  list.querySelector('.empty')?.remove();
  const li = document.createElement('li');
  li.className = kind;
  const time = document.createElement('time');
  time.textContent = new Date().toLocaleTimeString();
  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  icon.setAttribute('viewBox', '0 0 24 24');
  icon.setAttribute('fill', 'none');
  icon.setAttribute('stroke', 'currentColor');
  icon.setAttribute('stroke-width', '1.8');
  icon.setAttribute('stroke-linecap', 'round');
  icon.setAttribute('stroke-linejoin', 'round');
  icon.setAttribute('aria-hidden', 'true');
  icon.classList.add('icon');
  icon.innerHTML = ICONS[kind];
  const span = document.createElement('span');
  span.textContent = text;
  li.append(time, icon, span);
  list.prepend(li);
  while (list.children.length > 50) list.lastChild.remove();
}

const fileName = (path) => path.split(/[\\/]/).pop();

// Readable game name: the name Laurel found for the game, else the engine's hooked
// window title without the invisible characters some games pad it with, else the
// executable.
function gameName(game) {
  if (!game) return null;
  if (game.name) return game.name;
  const title = (game.title ?? '').replace(/[\u0000-\u001f­᠎​-‏‪-‮⁠-⁤⁦-⁩﻿]/g, '').replace(/\s+/g, ' ').trim();
  return title || (game.exe ?? '').replace(/\.exe$/i, '') || null;
}

// Views: library (home), viewer (opened from the library), settings.
function showView(view) {
  const rail = view === 'viewer' ? 'library' : view;
  for (const button of document.querySelectorAll('.rail-btn[data-view]')) {
    const active = button.dataset.view === rail;
    button.classList.toggle('active', active);
    if (active) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current');
  }
  for (const name of ['library', 'viewer', 'settings']) $(`view-${name}`).hidden = name !== view;
}

for (const button of document.querySelectorAll('.rail-btn[data-view]')) {
  button.onclick = () => {
    if (button.dataset.view === 'library') window.openLibrary?.();
    else window.openSettings?.();
  };
}

// Window buttons (the title bar is Laurel's own); closing keeps Laurel in the tray.
$('win-min').onclick = () => appWindow.minimize();
$('win-max').onclick = () => appWindow.toggleMaximize();
$('win-close').onclick = () => appWindow.close();
const MAXIMIZE_ICON = '<rect x="5" y="5" width="14" height="14" rx="1"/>';
const RESTORE_ICON = '<rect x="5" y="8" width="11" height="11" rx="1"/><path d="M8 8V5h11v11h-3"/>';
async function showMaximized() {
  const maximized = await appWindow.isMaximized();
  $('win-max').querySelector('svg').innerHTML = maximized ? RESTORE_ICON : MAXIMIZE_ICON;
  $('win-max').setAttribute('aria-label', maximized ? 'Restore' : 'Maximize');
  $('win-max').title = maximized ? 'Restore' : 'Maximize';
}
appWindow.onResized(showMaximized);
showMaximized();

const ENCODERS = {
  obs_nvenc_h264_tex: 'NVIDIA NVENC H.264',
  obs_nvenc_hevc_tex: 'NVIDIA NVENC HEVC',
  obs_nvenc_av1_tex: 'NVIDIA NVENC AV1',
  obs_x264: 'x264 (CPU)',
};

let engineState = { state: 'starting' };
let pausedForDisk = false;
// Game detection (spec game.detection): the game being recorded, if any.
let gameState = { mode: 'detect', game: null, recording: false };

// The recorder state in the title bar.
function showStatus() {
  const chip = $('status-chip');
  let state = engineState.state;
  let text = { starting: 'Starting', stopped: 'Engine stopped', failed: 'Engine failed' }[state] ?? state;
  if (state === 'ready') {
    if (gameState.enabled === false) [state, text] = ['off', 'Recording is off'];
    else if (pausedForDisk) [state, text] = ['paused', 'Paused: disk almost full'];
    else if (gameState.needsAdmin) [state, text] = ['admin', `${gameState.game?.name ?? 'The game'} needs administrator`];
    else if (gameState.capture === 'screen') text = 'Recording the screen';
    else if (gameState.game) text = `Recording ${gameState.game.name}`;
    else if (gameState.mode === 'detect') [state, text] = ['waiting', 'Waiting for a game'];
    else text = 'Recording';
  }
  chip.className = `status ${state}`;
  chip.textContent = text;
  const canSave = engineState.state === 'ready' && gameState.recording;
  for (const id of ['save-clip', 'screenshot']) $(id).disabled = !canSave;
}

// The status chip opens the details: Settings → Advanced.
$('status-chip').onclick = () => window.openSettings?.('advanced');

function showEngineInfo() {
  if (!engineInfo) return;
  $('engine-info').textContent = `Laurel ${window.laurelVersion ?? ''} · libobs ${engineInfo.version}${gameState.elevated ? ' · administrator' : ''}`;
  $('about-libobs').textContent = engineInfo.version;
}

function showGame(g) {
  gameState = g;
  showStatus();
  for (const input of document.querySelectorAll('.rec-input')) input.checked = g.enabled !== false;
  $('capture-hint').hidden = !(g.enabled !== false && g.mode === 'detect' && !g.game && g.capture !== 'screen');
  const idle = g.enabled === false ? 'Recording is off' : (g.mode === 'detect' && !g.game && g.capture !== 'screen' ? 'Waiting for a game' : null);
  if (idle) {
    $('capture-state').textContent = idle;
    $('capture-state').classList.remove('live');
  }
  // A game that runs as administrator needs Laurel to run as administrator too.
  $('admin-banner').hidden = !g.needsAdmin;
  if (g.needsAdmin && g.game) $('admin-title').textContent = `${g.game.name} runs as administrator`;
  showEngineInfo();
}

function showState(s) {
  engineState = s;
  showStatus();
  $('restart').hidden = s.state !== 'failed';
  if (s.state === 'ready') {
    engineInfo = s.info;
    $('engine-detail').textContent = `Last ${s.info.bufferSecs} s · ${s.info.output.join('x')} · ${s.info.fps} fps`;
    $('s-buffer').textContent = `${s.info.bufferSecs} s`;
    $('s-encoder').textContent = ENCODERS[s.info.encoder] ?? s.info.encoder;
    showEngineInfo();
  } else {
    $('engine-detail').textContent = s.message ?? 'Waiting for the engine';
  }
}

// Pipeline stats on the Advanced page, while it is open.
async function refreshStats() {
  if (!engineInfo || $('view-settings').hidden || document.querySelector('.page[data-page="advanced"]').hidden) return;
  try {
    const s = await invoke('engine_request', { method: 'status' });
    // D13: Laurel captures the game on its own; the target is not a user choice.
    const live = s.captureSize[0] > 0;
    const game = gameState.game?.name ?? gameName(s.game);
    let text = gameState.enabled === false ? 'Recording is off' : 'Waiting for a game';
    if (live && gameState.capture === 'screen') text = `Recording the screen at ${s.captureSize.join('x')}`;
    else if (live && gameState.capture === 'window') text = `Capturing ${game ?? 'the game'} by its window at ${s.captureSize.join('x')}`;
    else if (live) text = `Capturing ${game ?? 'the game'} at ${s.captureSize.join('x')}`;
    else if (gameState.game && gameState.enabled !== false) text = `${gameState.game.name} found, starting the capture`;
    $('capture-state').textContent = text;
    $('capture-state').classList.toggle('live', live);
    $('s-capture').textContent = live ? s.captureSize.join('x') : 'no image';
    $('s-fps').textContent = s.activeFps.toFixed(1);
    $('s-frames').textContent = s.totalFrames.toLocaleString('en-US');
    $('s-dropped').textContent = `${s.skippedFrames} / ${s.laggedFrames}`;
    // D12: track 2 is the game's own audio, following the hooked game.
    $('s-audio').textContent = s.audio?.mode === 'game' ? (s.audio.follows ?? 'waiting for a game') : (s.audio?.mode ?? '–');
  } catch {
    // The engine may be restarting; the state event explains it.
  }
}

$('save-clip').onclick = () => invoke('save_clip_now');
$('screenshot').onclick = () => invoke('screenshot_now');
// Spec record.toggle: off releases the capture and the replay buffer.
for (const input of document.querySelectorAll('.rec-input')) {
  input.onchange = async () => {
    const enabled = input.checked;
    showGame(await invoke('set_recording', { enabled }));
    activity(enabled ? 'Recording turned on' : 'Recording turned off');
  };
}
$('admin-restart').onclick = async () => {
  $('admin-restart').disabled = true;
  try {
    await invoke('restart_as_admin');
  } catch (e) {
    activity(e === 'cancelled' ? 'Restart as administrator cancelled' : `Could not restart as administrator: ${e}`, 'bad');
    $('admin-restart').disabled = false;
  }
};
$('restart').onclick = () => invoke('engine_restart');

listen('engine-state', (e) => {
  showState(e.payload);
  if (e.payload.state !== 'ready' && e.payload.message) activity(`Engine ${e.payload.state}: ${e.payload.message}`, e.payload.state === 'failed' ? 'bad' : 'info');
});
listen('clip-saved', (e) => activity(`Clip saved in ${e.payload.saveMs} ms: ${fileName(e.payload.path)}`, 'clip'));
listen('screenshot-saved', (e) => activity(`Screenshot saved: ${fileName(e.payload.path)}`, 'shot'));
listen('action-failed', (e) => activity(`${e.payload.action} failed: ${e.payload.message}`, 'bad'));
listen('storage-state', (e) => { pausedForDisk = e.payload.pausedForDisk; showStatus(); });
listen('storage-pruned', (e) => activity(`Storage limit: ${e.payload.paths.length} old item(s) removed`, 'info'));
listen('game-state', (e) => {
  const before = gameState.game;
  showGame(e.payload);
  const now = e.payload.game;
  if (before && before.key !== now?.key) activity(`${before.name} closed`);
  if (now && now.key !== before?.key) activity(`Recording ${now.name}`);
});

window.__TAURI__.app.getVersion().then((v) => {
  window.laurelVersion = v;
  $('about-version').textContent = v;
  showEngineInfo();
});
invoke('engine_state').then(showState);
invoke('game_state').then(showGame);
setInterval(refreshStats, 2000);

// The view the window opens in (development: LAUREL_START_VIEW), once every
// script has set its view up.
window.addEventListener('DOMContentLoaded', async () => {
  const view = (await invoke('start_view')) ?? 'library';
  const [name, page] = view.split(':');
  if (name === 'settings') window.openSettings?.(page);
  else window.openLibrary?.({ clips: 'clip', screenshots: 'screenshot' }[name] ?? 'all');
});
