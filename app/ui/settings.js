// Settings (storage.location, storage.limits, app.startup, overlay.in-game,
// alerts.sound, capture.cursor, capture.screen, quality.encoding,
// audio.separate-tracks): pages picked in the side panel,
// which a search narrows down, and the first-run "start with Windows?" banner.
// The backend saves the whole settings object, so every save sends all fields.
let settings = null;
let settingsPage = 'recording';

const pages = () => [...document.querySelectorAll('#view-settings .page')];

function showPage(page) {
  if (!pages().some((p) => p.dataset.page === page)) page = 'recording';
  settingsPage = page;
  for (const section of pages()) section.hidden = section.dataset.page !== page;
  if (page === 'about') loadLicenses();
  for (const button of document.querySelectorAll('#view-settings .side-item[data-page]')) {
    button.classList.toggle('active', button.dataset.page === page);
    if (button.dataset.page === page) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current');
  }
  document.querySelector('.settings-pane').scrollTop = 0;
}

for (const button of document.querySelectorAll('#view-settings .side-item[data-page]')) {
  button.onclick = () => showPage(button.dataset.page);
}

// Search: keeps the pages whose text (names, descriptions, buttons) has every word.
$('settings-search').oninput = () => {
  const words = $('settings-search').value.toLowerCase().split(/\s+/).filter(Boolean);
  const found = new Set(pages().filter((p) => words.every((w) => p.textContent.toLowerCase().includes(w))).map((p) => p.dataset.page));
  for (const button of document.querySelectorAll('#view-settings .side-item[data-page]')) button.hidden = !found.has(button.dataset.page);
  for (const group of document.querySelectorAll('#view-settings .side-group')) group.hidden = !group.querySelector('.side-item:not([hidden])');
  $('settings-no-match').hidden = found.size > 0;
  if (found.size && !found.has(settingsPage)) showPage([...found][0]);
};

const GB = 1024 ** 3;
const fmtGb = (bytes) => `${(bytes / GB).toFixed(1)} GB`;

function namingPreview(pattern) {
  const d = new Date();
  const two = (n) => String(n).padStart(2, '0');
  const name = pattern
    .replaceAll('{game}', 'Remnant From the Ashes')
    .replaceAll('{date}', `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`)
    .replaceAll('{time}', `${two(d.getHours())}-${two(d.getMinutes())}-${two(d.getSeconds())}`);
  return `${name.replace(/[<>:"/\\|?*]/g, ' ').replace(/\s+/g, ' ').trim() || 'Laurel'}.mp4`;
}

function showStorage(storage) {
  $('opt-folder').textContent = storage.folder;
  const parts = [`${fmtGb(storage.usedBytes)} used`];
  if (settings.limitEnabled) parts[0] += ` of ${settings.limitGb} GB`;
  if (storage.freeBytes != null) parts.push(`${fmtGb(storage.freeBytes)} free on the drive`);
  if (storage.pausedForDisk) parts.push('recording paused: disk almost full');
  $('storage-usage').textContent = parts.join(' · ');
  window.showStorageBar?.(storage, settings.limitEnabled ? settings.limitGb : 0);
  pausedForDisk = storage.pausedForDisk;
  showStatus();
}

// Video (spec quality.encoding): the encoders and codecs this PC has come from the
// engine, and the resolutions go up to the monitor's.
const FAMILIES = { nvenc: 'NVIDIA NVENC', amf: 'AMD AMF', qsv: 'Intel Quick Sync', x264: 'Processor (x264)' };

function setSelect(select, value) {
  if (![...select.options].some((o) => o.value === String(value))) select.append(new Option(String(value), String(value)));
  select.value = String(value);
}

function fillVideo(s, info = engineInfo) {
  const encoders = info?.encoders ?? [];
  const families = [...new Set(encoders.map((e) => e.family))];
  const options = [['auto', 'Auto'], ...families.map((f) => [f, FAMILIES[f] ?? f])];
  if (s.videoEncoder !== 'auto' && encoders.length && !families.includes(s.videoEncoder)) {
    options.push([s.videoEncoder, `${FAMILIES[s.videoEncoder] ?? s.videoEncoder} (not on this PC)`]);
  }
  $('opt-video-encoder').replaceChildren(...options.map(([value, text]) => new Option(text, value)));
  setSelect($('opt-video-encoder'), s.videoEncoder);
  // The codecs of the chosen encoder (of any, for Auto); H.264 is always there.
  const pool = s.videoEncoder === 'auto' ? encoders : encoders.filter((e) => e.family === s.videoEncoder);
  for (const option of $('opt-video-codec').options) {
    option.disabled = option.value !== 'h264' && encoders.length > 0 && !pool.some((e) => e.codec === option.value);
  }
  $('opt-video-codec').value = s.videoCodec;
  const base = info?.base ?? [0, 0];
  const heights = [2160, 1440, 1080, 720].filter((h) => !base[1] || h < base[1]);
  $('opt-video-height').replaceChildren(new Option(base[1] ? `Monitor (${base[0]}x${base[1]})` : 'Monitor', '0'), ...heights.map((h) => new Option(`${h}p`, String(h))));
  setSelect($('opt-video-height'), s.videoHeight);
  setSelect($('opt-video-fps'), s.videoFps);
  setSelect($('opt-clip-secs'), s.clipSecs);
  $('opt-video-preset').value = s.videoPreset;
  $('opt-video-rate').value = s.videoRate;
  $('opt-video-bitrate').value = s.videoBitrate;
}

// What the clips' main track holds (D12 revised): the one Laurel, Discord and
// most players play. All PC sound takes everything the speakers play.
const MIX_DESC = {
  game: 'What you hear when a clip plays in Laurel, Discord or any player. Only the game: your voice stays on its own track.',
  'game-mic': 'What you hear when a clip plays in Laurel, Discord or any player: the game and your voice.',
  'pc-mic': 'Everything your PC plays goes into clips: Discord, music, notifications and Laurel\'s own sounds, plus your voice.',
};

function showMix(mix) {
  $('audio-mix-desc').textContent = MIX_DESC[mix] ?? MIX_DESC['game-mic'];
  $('audio-mix-desc').classList.toggle('warn', mix === 'pc-mic');
}

// Options that only apply when another one is on stay visible but dimmed.
function dim(row, off) {
  row.classList.toggle('dim', off);
  for (const input of row.querySelectorAll('input, select')) input.disabled = off;
}

function fill(s) {
  settings = s;
  $('opt-overlay').checked = s.overlay;
  $('opt-sounds').checked = s.sounds;
  $('opt-volume').value = s.volume;
  $('opt-volume-value').textContent = s.volume;
  $('opt-naming').value = s.naming;
  $('naming-preview').textContent = namingPreview(s.naming);
  $('opt-limit').checked = s.limitEnabled;
  $('opt-limit-gb').value = s.limitGb;
  $('opt-recycle').checked = s.recycle;
  $('opt-min-free').value = s.minFreeGb;
  $('opt-cursor').checked = s.showCursor;
  $('opt-screen').checked = s.screenRecording;
  $('opt-screen-switch').checked = s.screenGameSwitch;
  for (const row of document.querySelectorAll('.screen-option')) dim(row, !s.screenRecording);
  $('opt-limit-gb').disabled = !s.limitEnabled;
  fillVideo(s);
  $('opt-audio-mix').value = s.audioMix;
  $('opt-updates').checked = s.checkUpdates;
  showMix(s.audioMix);
  fillMonitors(s.screenMonitor);
  $('opt-autostart').checked = s.autostart;
  $('startup-banner').hidden = s.autostartAsked;
  showStartupBanner(s.autostart);
  showStorage(s.storage);
}

// Monitors for screen recording (spec capture.screen), from the engine; the empty
// value is the primary monitor, whichever it is.
async function fillMonitors(selected) {
  const select = $('opt-screen-monitor');
  let monitors = [];
  try {
    monitors = (await invoke('engine_request', { method: 'listTargets' })).filter((t) => t.target.startsWith('monitor:'));
  } catch {
    // The engine may be restarting; keep the primary monitor option.
  }
  const primary = document.createElement('option');
  primary.value = '';
  primary.textContent = 'Primary monitor';
  select.replaceChildren(primary, ...monitors.map((m) => {
    const option = document.createElement('option');
    option.value = m.id;
    option.textContent = m.label.replace(/^Monitor: /, '');
    return option;
  }));
  select.value = monitors.some((m) => m.id === selected) ? selected : '';
}

async function loadSettings() {
  fill(await invoke('get_settings'));
}

async function saveSettings() {
  const next = {
    ...settings,
    overlay: $('opt-overlay').checked,
    sounds: $('opt-sounds').checked,
    volume: Number($('opt-volume').value),
    naming: $('opt-naming').value,
    limitEnabled: $('opt-limit').checked,
    limitGb: Number($('opt-limit-gb').value) || settings.limitGb,
    recycle: $('opt-recycle').checked,
    minFreeGb: Number($('opt-min-free').value),
    showCursor: $('opt-cursor').checked,
    screenRecording: $('opt-screen').checked,
    screenMonitor: $('opt-screen-monitor').value,
    screenGameSwitch: $('opt-screen-switch').checked,
    videoEncoder: $('opt-video-encoder').value,
    // A codec the chosen encoder lacks becomes H.264.
    videoCodec: $('opt-video-codec').selectedOptions[0]?.disabled ? 'h264' : $('opt-video-codec').value,
    videoPreset: $('opt-video-preset').value,
    videoRate: $('opt-video-rate').value,
    videoBitrate: Number($('opt-video-bitrate').value) || settings.videoBitrate,
    videoHeight: Number($('opt-video-height').value),
    videoFps: Number($('opt-video-fps').value),
    clipSecs: Number($('opt-clip-secs').value),
    audioMix: $('opt-audio-mix').value,
    checkUpdates: $('opt-updates').checked,
  };
  try {
    fill(await invoke('set_settings', { settings: next }));
    $('settings-status').textContent = 'Saved';
    setTimeout(() => { $('settings-status').textContent = ''; }, 1500);
  } catch (e) {
    $('settings-status').textContent = `Could not save: ${e}`;
  }
}

for (const id of ['opt-overlay', 'opt-sounds', 'opt-volume', 'opt-naming', 'opt-limit', 'opt-limit-gb', 'opt-recycle', 'opt-min-free', 'opt-cursor', 'opt-screen', 'opt-screen-monitor', 'opt-screen-switch',
  'opt-video-encoder', 'opt-video-codec', 'opt-video-preset', 'opt-video-rate', 'opt-video-bitrate', 'opt-video-height', 'opt-video-fps', 'opt-clip-secs', 'opt-audio-mix', 'opt-updates']) {
  $(id).addEventListener('change', saveSettings);
}
$('opt-volume').oninput = () => { $('opt-volume-value').textContent = $('opt-volume').value; };
$('opt-naming').oninput = () => { $('naming-preview').textContent = namingPreview($('opt-naming').value); };
$('preview-clip').onclick = () => invoke('preview_notice', { kind: 'clip' });
$('preview-error').onclick = () => invoke('preview_notice', { kind: 'error' });

// First run (app.startup): the installer turns start with Windows on, and the banner
// confirms it; without the installer it asks.
function showStartupBanner(on) {
  $('banner-title').textContent = on ? 'Laurel starts with Windows' : 'Start Laurel with Windows?';
  $('banner-text').textContent = on
    ? 'It waits in the tray, ready to record when you open a game. You can change this later in Settings, Startup.'
    : 'Laurel then waits in the tray, ready to record when you open a game.';
  $('banner-yes').textContent = on ? 'Keep it on' : 'Start with Windows';
  $('banner-no').textContent = on ? 'Turn it off' : 'Not now';
}

// Startup (app.startup): the Run key is the source of truth.
async function setAutostart(enabled) {
  try {
    fill(await invoke('set_autostart', { enabled }));
  } catch (e) {
    $('settings-status').textContent = `Could not change startup: ${e}`;
  }
}
$('opt-autostart').onchange = () => setAutostart($('opt-autostart').checked);
$('banner-yes').onclick = () => setAutostart(true);
$('banner-no').onclick = () => setAutostart(false);

// Capture folder (storage.location): pick, then ask whether existing items move.
let pendingFolder = null;

$('change-folder').onclick = async () => {
  const folder = await invoke('pick_folder');
  if (!folder || folder === settings.storage.folder) return;
  pendingFolder = folder;
  const items = (await invoke('library_list', {})).length;
  $('move-question').textContent = items
    ? `Move your ${items} clips and screenshots to ${folder}?`
    : `Use ${folder} for new clips and screenshots?`;
  $('move-no').hidden = items === 0;
  $('move-yes').textContent = items ? 'Move them' : 'Use this folder';
  $('move-confirm').hidden = false;
};

async function applyFolder(moveItems) {
  $('move-confirm').hidden = true;
  $('settings-status').textContent = moveItems ? 'Moving your files…' : 'Switching folders…';
  try {
    const result = await invoke('change_folder', { folder: pendingFolder, moveItems });
    $('settings-status').textContent = result.failed.length
      ? `Moved ${result.moved}; ${result.failed.length} could not be moved`
      : moveItems ? `Moved ${result.moved} files` : 'Folder changed';
    activity(`Capture folder changed to ${pendingFolder}`);
  } catch (e) {
    $('settings-status').textContent = `${e}`;
  }
  pendingFolder = null;
  loadSettings();
}

$('move-yes').onclick = () => applyFolder(true);
$('move-no').onclick = () => applyFolder(false);
$('move-cancel').onclick = () => { $('move-confirm').hidden = true; pendingFolder = null; };

// About (the GPL's legal notices): the license texts come from the app and from the
// runtime next to the engine; the font licenses ship with the interface.
const FONT_LICENSES = [['OFL-Cinzel.txt', 'Cinzel'], ['OFL-Figtree.txt', 'Figtree'], ['OFL-JetBrainsMono.txt', 'JetBrains Mono']];
let licenseDocs = null;

async function loadLicenses() {
  if (licenseDocs) return;
  licenseDocs = await invoke('licenses');
  for (const [file, font] of FONT_LICENSES) {
    try {
      licenseDocs.push({ title: `${font} font: SIL Open Font License`, text: await (await fetch(`fonts/${file}`)).text() });
    } catch {
      // The interface still lists the others.
    }
  }
  $('license-doc').replaceChildren(...licenseDocs.map((d, i) => new Option(d.title, String(i))));
  showLicense();
}

function showLicense() {
  $('license-text').textContent = licenseDocs?.[Number($('license-doc').value)]?.text ?? '';
  $('license-text').scrollTop = 0;
}

$('license-doc').onchange = showLicense;
$('about-source').onclick = () => invoke('open_source_page');
$('open-logs').onclick = () => invoke('open_logs').catch((e) => { $('settings-status').textContent = `${e}`; });

// Updates (stage 6 plan): the check runs a little after the start when the setting is
// on, or here on demand; installing is always the user's choice.
let updateFound = null;

function showUpdate(text, found = updateFound) {
  updateFound = found;
  $('update-status').textContent = text;
  $('update-install').hidden = !found;
}

async function checkUpdate() {
  $('update-check').disabled = true;
  showUpdate('Checking…', null);
  try {
    const r = await invoke('update_check');
    showUpdate(r.available ? `Laurel ${r.version} is available (you have ${r.current}).` : `Laurel ${r.current} is up to date.`, r.available ? r : null);
  } catch (e) {
    showUpdate(`Could not check: ${e}`, null);
  }
  $('update-check').disabled = false;
}

async function installUpdate() {
  $('update-install').disabled = true;
  $('update-now').disabled = true;
  showUpdate(`Downloading Laurel ${updateFound?.version ?? ''}…`);
  try {
    await invoke('update_install');
  } catch (e) {
    showUpdate(`Could not install: ${e}`, null);
    $('update-text').textContent = `Could not install: ${e}`;
  }
  $('update-install').disabled = false;
  $('update-now').disabled = false;
}

$('update-check').onclick = checkUpdate;
$('update-install').onclick = installUpdate;
$('update-now').onclick = installUpdate;
$('update-later').onclick = () => { $('update-banner').hidden = true; };
listen('update-available', (e) => {
  showUpdate(`Laurel ${e.payload.version} is available (you have ${e.payload.current}).`, e.payload);
  $('update-title').textContent = `Laurel ${e.payload.version} is ready`;
  $('update-banner').hidden = false;
});
listen('update-progress', (e) => {
  const { received, total } = e.payload;
  const text = total ? `Downloading… ${Math.round((received / total) * 100)}%` : `Downloading… ${(received / 1048576).toFixed(1)} MB`;
  $('update-status').textContent = text;
  $('update-text').textContent = text;
});
invoke('update_info').then((info) => {
  if (!info.configured) {
    $('update-status').textContent = 'Updates are not set up in this build.';
    $('update-check').disabled = true;
  }
});

window.openSettings = (page) => {
  closeViewer(false);
  showView('settings');
  if (page) showPage(page);
  loadSettings();
};
showPage('recording');
listen('storage-state', (e) => settings && showStorage(e.payload));
listen('engine-state', (e) => { if (settings && e.payload.state === 'ready') fillVideo(settings, e.payload.info); });
listen('library-changed', () => { if (!$('view-settings').hidden) loadSettings(); });

loadSettings();
