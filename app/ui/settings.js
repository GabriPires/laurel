// Settings (storage.location, storage.limits, app.startup, overlay.in-game,
// alerts.sound, capture.cursor, capture.screen): pages picked in the side panel,
// which a search narrows down, and the first-run "start with Windows?" banner.
// The backend saves the whole settings object, so every save sends all fields.
let settings = null;
let settingsPage = 'recording';

const pages = () => [...document.querySelectorAll('#view-settings .page')];

function showPage(page) {
  if (!pages().some((p) => p.dataset.page === page)) page = 'recording';
  settingsPage = page;
  for (const section of pages()) section.hidden = section.dataset.page !== page;
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
  fillMonitors(s.screenMonitor);
  $('opt-autostart').checked = s.autostart;
  $('startup-banner').hidden = s.autostartAsked;
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
  };
  try {
    fill(await invoke('set_settings', { settings: next }));
    $('settings-status').textContent = 'Saved';
    setTimeout(() => { $('settings-status').textContent = ''; }, 1500);
  } catch (e) {
    $('settings-status').textContent = `Could not save: ${e}`;
  }
}

for (const id of ['opt-overlay', 'opt-sounds', 'opt-volume', 'opt-naming', 'opt-limit', 'opt-limit-gb', 'opt-recycle', 'opt-min-free', 'opt-cursor', 'opt-screen', 'opt-screen-monitor', 'opt-screen-switch']) {
  $(id).addEventListener('change', saveSettings);
}
$('opt-volume').oninput = () => { $('opt-volume-value').textContent = $('opt-volume').value; };
$('opt-naming').oninput = () => { $('naming-preview').textContent = namingPreview($('opt-naming').value); };
$('preview-clip').onclick = () => invoke('preview_notice', { kind: 'clip' });
$('preview-error').onclick = () => invoke('preview_notice', { kind: 'error' });

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

window.openSettings = (page) => {
  closeViewer(false);
  showView('settings');
  if (page) showPage(page);
  loadSettings();
};
showPage('recording');
listen('storage-state', (e) => settings && showStorage(e.payload));
listen('library-changed', () => { if (!$('view-settings').hidden) loadSettings(); });

loadSettings();
