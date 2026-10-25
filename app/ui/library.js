// Library, the home screen (specs library.browse, library.thumbnails, edit.trim,
// share.local, library.trash): a side panel with All, Clips, Screenshots, Kept,
// Recently deleted, the games and the storage used; a grid of clips and screenshots
// grouped by day, sorted and searched, with a muted preview of a clip under the
// pointer, where several items can be selected and kept, deleted or restored at
// once; and the viewer, with Laurel's own player, the previous and next item, the
// trim under the video and the details and actions on the right, Delete included.
// Deleted items wait in Recently deleted for the days chosen in Storage, and Undo
// brings them back right after a delete.
const { convertFileSrc } = window.__TAURI__.core;

const library = { items: [], deleted: [], filter: { type: 'all' }, sort: 'new', query: '', current: null, selected: new Set(), anchor: null, confirm: null };

const typeTitle = (type) => ({ all: t('All'), clip: t('Clips'), screenshot: t('Screenshots'), kept: t('Kept'), deleted: t('Recently deleted') })[type];
const DAY_MS = 86400000;
const deletedDays = () => settings?.deletedDays ?? 7;
// Whole days before a deleted item goes for good, at least one.
const daysLeft = (item) => Math.max(1, Math.ceil((item.deletedMs + deletedDays() * DAY_MS - Date.now()) / DAY_MS));
const inTrash = () => library.filter.type === 'deleted';
// Where items deleted for good end up, by the Storage setting.
const goneNote = () => (settings?.recycle === false ? t('This cannot be undone.') : t('They go to the Windows Recycle Bin.'));
const UNKNOWN_GAME = t('Unknown game');

function fmtDuration(ms) {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function fmtSeconds(sec) {
  const m = Math.floor(sec / 60);
  return `${m}:${(sec - m * 60).toFixed(1).padStart(4, '0').replace('.', DECIMAL)}`;
}

// Seconds with hundredths, for the trim marks.
function fmtSeconds2(sec) {
  const m = Math.floor(sec / 60);
  return `${m}:${(sec - m * 60).toFixed(2).padStart(5, '0').replace('.', DECIMAL)}`;
}

const two = (n) => String(n).padStart(2, '0');
const fmtTime = (d) => `${two(d.getHours())}:${two(d.getMinutes())}`;

// Today, Yesterday, "Oct 21", or "Oct 21, 2025" for another year, in the
// interface language.
const DAY = new Intl.DateTimeFormat(LOCALE, { month: 'short', day: 'numeric' });
const DAY_YEAR = new Intl.DateTimeFormat(LOCALE, { month: 'short', day: 'numeric', year: 'numeric' });
function fmtDay(ms) {
  const d = new Date(ms);
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return t('Today');
  if (d.toDateString() === yesterday.toDateString()) return t('Yesterday');
  return (d.getFullYear() === today.getFullYear() ? DAY : DAY_YEAR).format(d);
}

const fmtWhen = (ms) => `${fmtDay(ms)} ${fmtTime(new Date(ms))}`;

function fmtSize(bytes) {
  const mb = bytes / 1048576;
  const gb = mb / 1024;
  if (gb >= 100) return `${fmtNum(gb)} GB`;
  if (mb >= 1024) return `${fmtNum(gb, 1)} GB`;
  return mb >= 10 ? `${fmtNum(mb)} MB` : `${fmtNum(mb, 1)} MB`;
}

// The card's second line; the day is left out under a day heading.
function describe(item, withDay) {
  const when = withDay ? fmtWhen(item.createdMs) : fmtTime(new Date(item.createdMs));
  if (item.kind === 'clip') {
    const fps = item.fps ? Math.round(item.fps) : '';
    return `${when} · ${item.height}p${fps} · ${fmtSize(item.size)}`;
  }
  return `${when} · ${item.width}x${item.height} · ${fmtSize(item.size)}`;
}

// Screen recordings are stored under the game name "Screen" (spec capture.screen).
const gameOf = (item) => (item.game === 'Screen' ? t('Screen') : item.game ?? UNKNOWN_GAME);
const initial = (name) => (name.match(/[\p{L}\p{N}]/u)?.[0] ?? '?').toUpperCase();
// One game whatever the case, spaces and symbols ("Overwatch" and "Overwatch®").
const gameKey = (item) => gameOf(item).toLowerCase().replace(/[^\p{L}\p{N}]/gu, '') || gameOf(item);

function matches(item, filter) {
  if (filter.type === 'clip' || filter.type === 'screenshot') return item.kind === filter.type;
  if (filter.type === 'kept') return item.kept;
  if (filter.type === 'game') return gameKey(item) === filter.key;
  return true;
}

// The games with items, most items first; each is shown by the name of its most
// recent item.
function libraryGames() {
  const games = new Map();
  for (const item of library.items) {
    const game = games.get(gameKey(item)) ?? { key: gameKey(item), name: gameOf(item), count: 0, newest: 0 };
    game.count += 1;
    if (item.createdMs > game.newest) [game.newest, game.name] = [item.createdMs, gameOf(item)];
    games.set(game.key, game);
  }
  return [...games.values()].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

// The items the grid shows, in its order; the viewer steps through the same list.
// Recently deleted is sorted and grouped by when items were deleted.
const sortTime = (item) => item.deletedMs ?? item.createdMs;
function visible() {
  const query = library.query.trim().toLowerCase();
  const source = inTrash() ? library.deleted : library.items;
  const items = source.filter((item) => (inTrash() || matches(item, library.filter))
    && (!query || gameOf(item).toLowerCase().includes(query) || fileName(item.path).toLowerCase().includes(query)));
  if (library.sort === 'old') items.sort((a, b) => sortTime(a) - sortTime(b));
  else if (library.sort === 'size') items.sort((a, b) => b.size - a.size);
  else items.sort((a, b) => sortTime(b) - sortTime(a));
  return items;
}

function svgIcon(paths, size = 16) {
  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const [k, v] of Object.entries({ width: size, height: size, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.8', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true' })) icon.setAttribute(k, v);
  icon.innerHTML = paths;
  return icon;
}

function renderSide() {
  const counts = { all: library.items.length, clip: 0, screenshot: 0, kept: 0, deleted: library.deleted.length };
  for (const item of library.items) {
    counts[item.kind] += 1;
    if (item.kept) counts.kept += 1;
  }
  for (const [type, n] of Object.entries(counts)) $(`count-${type}`).textContent = n || '';
  for (const button of document.querySelectorAll('#view-library .side-item[data-filter]')) {
    button.classList.toggle('active', library.filter.type === button.dataset.filter);
  }
  $('side-games').replaceChildren(...libraryGames().map((game) => {
    const button = document.createElement('button');
    button.className = 'side-item';
    button.classList.toggle('active', library.filter.type === 'game' && library.filter.key === game.key);
    const tile = document.createElement('span');
    tile.className = 'game-tile';
    tile.textContent = initial(game.name);
    const name = document.createElement('span');
    name.className = 'side-name';
    name.textContent = game.name;
    name.title = game.name;
    const count = document.createElement('span');
    count.className = 'count';
    count.textContent = game.count;
    button.append(tile, name, count);
    button.onclick = () => setFilter({ type: 'game', key: game.key, name: game.name });
    return button;
  }));
}

// Preview on hover (a friends' suggestion, 2026-10-24): after the pointer (or the
// keyboard focus) rests on a clip, it plays muted inside its card, from the middle
// of longer clips, so look-alike thumbnails are told apart. One card at a time.
const PREVIEW_DELAY = 450;
const preview = { card: null, timer: 0 };

function stopPreview() {
  clearTimeout(preview.timer);
  const card = preview.card;
  preview.card = null;
  if (!card) return;
  card.classList.remove('previewing');
  const video = card.querySelector('.preview');
  if (video) {
    video.pause();
    video.removeAttribute('src');
    video.load();
    video.remove();
  }
}

function startPreview(card, item) {
  if (preview.card === card) return;
  stopPreview();
  preview.card = card;
  preview.timer = setTimeout(() => {
    if (preview.card !== card) return;
    const video = document.createElement('video');
    video.className = 'preview';
    video.muted = true;
    video.playsInline = true;
    video.preload = 'auto';
    const total = (item.durationMs ?? 0) / 1000;
    const from = total > 15 ? total / 2 : 0;
    const play = () => { video.currentTime = from; video.play().catch(() => {}); };
    video.addEventListener('loadedmetadata', play, { once: true });
    video.addEventListener('ended', play);
    video.addEventListener('playing', () => video.classList.add('shown'), { once: true });
    video.src = convertFileSrc(item.path);
    card.querySelector('.thumb').append(video);
    card.classList.add('previewing');
  }, PREVIEW_DELAY);
}

const CHECK = '<path d="M5 12l4 4 10-10" stroke-width="3"/>';

function card(item, withDay) {
  const button = document.createElement('button');
  button.className = 'item';
  button.dataset.path = item.path;
  const thumb = document.createElement('div');
  thumb.className = 'thumb';
  if (item.thumbnail) thumb.style.backgroundImage = `url("${convertFileSrc(item.thumbnail)}")`;
  const box = document.createElement('span');
  box.className = 'select-box';
  box.setAttribute('aria-hidden', 'true');
  box.append(svgIcon(CHECK, 14));
  box.onclick = (e) => {
    e.stopPropagation();
    if (e.shiftKey && library.anchor) selectRange(item.path);
    else toggleSelect(item.path);
  };
  thumb.append(box);
  if (item.deletedMs != null) {
    const left = document.createElement('span');
    left.className = 'deleted-badge';
    left.textContent = tp(daysLeft(item), 'Goes in {n} day', 'Goes in {n} days');
    thumb.append(left);
  }
  if (item.kept) {
    const kept = document.createElement('span');
    kept.className = 'kept';
    kept.textContent = `★ ${t('Kept|state')}`;
    kept.title = t('Kept: never removed by the storage limit');
    thumb.append(kept);
  }
  if (item.kind === 'clip') {
    const play = document.createElement('span');
    play.className = 'play';
    const icon = svgIcon('<path d="M8 5l12 7-12 7z" fill="currentColor" stroke="none"/>', 14);
    play.append(icon);
    const badge = document.createElement('span');
    badge.className = 'badge';
    badge.textContent = fmtDuration(item.durationMs);
    thumb.append(play, badge);
  }
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = gameOf(item);
  const meta = document.createElement('span');
  meta.className = 'meta';
  meta.textContent = describe(item, withDay || item.deletedMs != null);
  button.append(thumb, name, meta);
  // A click opens the item; with Ctrl, or while items are selected, it selects;
  // with Shift it selects a range; a right click selects too.
  button.onclick = (e) => {
    if (e.shiftKey && library.anchor) selectRange(item.path);
    else if (e.ctrlKey || e.metaKey || library.selected.size) toggleSelect(item.path);
    else openViewer(item);
  };
  button.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    toggleSelect(item.path);
  });
  if (item.kind === 'clip') {
    const leave = () => { if (preview.card === button) stopPreview(); };
    button.addEventListener('pointerenter', () => startPreview(button, item));
    button.addEventListener('pointerleave', leave);
    button.addEventListener('focus', () => startPreview(button, item));
    button.addEventListener('blur', leave);
  }
  return button;
}

function emptyText() {
  if (library.query.trim()) return t('Nothing matches “{query}”.', { query: library.query.trim() });
  const clipKey = $('kbd-clip').textContent || 'F8';
  const shotKey = $('kbd-screenshot').textContent || 'Shift+F8';
  return {
    screenshot: t('No screenshots yet. Press {key} in a game.', { key: shotKey }),
    kept: t('Nothing kept yet. Keep a clip in the viewer and the storage limit never removes it.'),
    deleted: tp(deletedDays(), 'Nothing here. Deleted clips and screenshots wait here for {n} day before they go for good.', 'Nothing here. Deleted clips and screenshots wait here for {n} days before they go for good.'),
  }[library.filter.type] ?? t('No clips yet. Press {key} in a game to save the last seconds.', { key: clipKey });
}

function renderLibrary() {
  stopPreview();
  renderSide();
  const items = visible();
  // The selection keeps only what is shown.
  const shown = new Set(items.map((i) => i.path));
  library.selected = new Set([...library.selected].filter((path) => shown.has(path)));
  $('library-title').textContent = library.filter.type === 'game' ? library.filter.name : typeTitle(library.filter.type);
  $('library-count').textContent = tp(items.length, '{n} item', '{n} items');
  $('library-empty').hidden = items.length > 0;
  $('library-empty').textContent = emptyText();
  $('library-empty-trash').hidden = !inTrash() || library.deleted.length === 0;
  // Grouped by day when sorted by date.
  const groups = [];
  if (library.sort === 'size') {
    groups.push({ title: null, items });
  } else {
    for (const item of items) {
      const day = fmtDay(sortTime(item));
      if (groups.at(-1)?.title !== day) groups.push({ title: day, items: [] });
      groups.at(-1).items.push(item);
    }
  }
  $('library-groups').replaceChildren(...groups.map((group) => {
    const section = document.createElement('section');
    section.className = 'library-groups';
    if (group.title) {
      const head = document.createElement('div');
      head.className = 'group-head';
      const title = document.createElement('h2');
      title.className = 'group-title';
      title.textContent = group.title;
      const all = document.createElement('button');
      all.className = 'link-btn group-select';
      all.textContent = t('Select all');
      all.onclick = () => selectGroup(group.items);
      head.append(title, all);
      section.append(head);
    }
    const grid = document.createElement('div');
    grid.className = 'library-grid';
    grid.append(...group.items.map((item) => card(item, !group.title)));
    section.append(grid);
    return section;
  }));
  showSelection();
}

async function loadLibrary() {
  try {
    [library.items, library.deleted] = await Promise.all([invoke('library_list', {}), invoke('library_list', { deleted: true })]);
  } catch (e) {
    library.items = [];
    library.deleted = [];
    activity(t('Could not read the library: {error}', { error: e }), 'bad');
  }
}

function setFilter(filter) {
  library.filter = filter;
  library.selected.clear();
  library.confirm = null;
  closeViewer(false);
  renderLibrary();
  $('library-groups').closest('.pane').scrollTop = 0;
}

// Selection (a friends' suggestion, 2026-10-25): the cards follow it without a
// new render; the bar over the grid shows the count and the actions.
function showSelection() {
  const n = library.selected.size;
  $('library-groups').classList.toggle('selecting', n > 0);
  for (const item of document.querySelectorAll('#library-groups .item')) {
    const on = library.selected.has(item.dataset.path);
    item.classList.toggle('selected', on);
    if (n) item.setAttribute('aria-pressed', String(on));
    else item.removeAttribute('aria-pressed');
  }
  showBar();
}

const selectedItems = () => visible().filter((item) => library.selected.has(item.path));

function showBar() {
  const n = library.selected.size;
  const confirm = library.confirm;
  $('selection-bar').hidden = !confirm && n === 0;
  $('selection-bar').classList.toggle('danger', Boolean(confirm));
  $('selection-main').hidden = Boolean(confirm);
  $('selection-confirm').hidden = !confirm;
  if (confirm) {
    $('selection-question').textContent = confirm.question;
    $('sel-yes').textContent = confirm.yes;
    return;
  }
  $('selection-count').textContent = tp(n, '{n} selected', '{n} selected');
  for (const id of ['sel-keep', 'sel-delete']) $(id).hidden = inTrash();
  for (const id of ['sel-restore', 'sel-purge']) $(id).hidden = !inTrash();
  const chosen = selectedItems();
  $('sel-keep').textContent = chosen.length && chosen.every((i) => i.kept) ? t('Stop keeping') : t('Keep');
}

function toggleSelect(path) {
  if (!library.selected.delete(path)) library.selected.add(path);
  library.anchor = path;
  library.confirm = null;
  showSelection();
}

// Shift: everything between the last item clicked and this one.
function selectRange(path) {
  const list = visible().map((i) => i.path);
  const [a, b] = [list.indexOf(library.anchor), list.indexOf(path)];
  if (a < 0 || b < 0) return toggleSelect(path);
  for (const p of list.slice(Math.min(a, b), Math.max(a, b) + 1)) library.selected.add(p);
  library.confirm = null;
  showSelection();
}

// A day's "Select all": the whole day, or none of it when it already is.
function selectGroup(items) {
  const all = items.every((i) => library.selected.has(i.path));
  for (const i of items) {
    if (all) library.selected.delete(i.path);
    else library.selected.add(i.path);
  }
  library.confirm = null;
  showSelection();
}

function selectAll() {
  for (const item of visible()) library.selected.add(item.path);
  library.confirm = null;
  showSelection();
}

function clearSelection() {
  library.selected.clear();
  library.confirm = null;
  showSelection();
}

// A question in the bar over the grid; Esc or Cancel drops it.
function askConfirm(question, yes, run) {
  library.confirm = { question, yes, run };
  showBar();
  $('sel-yes').focus();
}

$('sel-yes').onclick = () => {
  const confirm = library.confirm;
  library.confirm = null;
  showBar();
  confirm?.run();
};
$('sel-no').onclick = () => { library.confirm = null; showBar(); };
$('sel-all').onclick = selectAll;
$('sel-cancel').onclick = clearSelection;
$('sel-keep').onclick = async () => {
  const chosen = selectedItems();
  const kept = !chosen.every((i) => i.kept);
  await invoke('library_keep', { paths: chosen.map((i) => i.path), kept });
  await loadLibrary();
  renderLibrary();
};
$('sel-delete').onclick = () => askDeleteSelection();
$('sel-restore').onclick = () => restoreItems([...library.selected]);
$('sel-purge').onclick = () => askDeleteSelection();
$('library-empty-trash').onclick = () => askConfirm(
  `${tp(library.deleted.length, 'Delete the {n} item in Recently deleted for good?', 'Delete the {n} items in Recently deleted for good?')} ${goneNote()}`,
  t('Empty'),
  () => purgeItems(null),
);

function askDeleteSelection() {
  const paths = [...library.selected];
  if (!paths.length) return;
  if (inTrash()) {
    askConfirm(`${tp(paths.length, 'Delete {n} item for good?', 'Delete {n} items for good?')} ${goneNote()}`, t('Delete now'), () => purgeItems(paths));
  } else {
    askConfirm(`${tp(paths.length, 'Delete {n} item?', 'Delete {n} items?')} ${waitNote()}`, t('Delete'), () => deleteItems(paths));
  }
}

const waitNote = () => tp(deletedDays(), 'Deleted items wait in Recently deleted for {n} day.', 'Deleted items wait in Recently deleted for {n} days.');

// Delete, restore and delete for good, for the grid and the viewer; true when
// at least one item moved.
async function deleteItems(paths) {
  let result;
  try {
    result = await invoke('library_delete', { paths });
  } catch (e) {
    toast(t('Could not delete: {error}', { error: e }));
    return false;
  }
  const moved = result.items.filter(Boolean).map((i) => i.path);
  const n = paths.length - result.failed.length;
  library.selected.clear();
  await loadLibrary();
  if ($('view-viewer').hidden) renderLibrary();
  if (result.failed.length) activity(t('Could not delete: {error}', { error: result.failed.join('; ') }), 'bad');
  if (n) {
    activity(tp(n, 'Moved {n} item to Recently deleted', 'Moved {n} items to Recently deleted'));
    toast(tp(n, '{n} item deleted', '{n} items deleted'), moved.length ? t('Undo') : null, moved.length ? () => restoreItems(moved) : null);
  }
  return n > 0;
}

async function restoreItems(paths) {
  let result;
  try {
    result = await invoke('library_restore', { paths });
  } catch (e) {
    toast(t('Could not restore: {error}', { error: e }));
    return false;
  }
  const n = result.items.length;
  library.selected.clear();
  await loadLibrary();
  if ($('view-viewer').hidden) renderLibrary();
  if (result.failed.length) activity(t('Could not restore: {error}', { error: result.failed.join('; ') }), 'bad');
  if (n) toast(tp(n, '{n} item restored', '{n} items restored'));
  return n > 0;
}

// paths null: everything in Recently deleted.
async function purgeItems(paths) {
  let result;
  try {
    result = await invoke('library_purge', paths ? { paths } : {});
  } catch (e) {
    toast(t('Could not delete: {error}', { error: e }));
    return false;
  }
  const n = result.items.length;
  library.selected.clear();
  await loadLibrary();
  if ($('view-viewer').hidden) renderLibrary();
  if (result.failed.length) activity(t('Could not delete: {error}', { error: result.failed.join('; ') }), 'bad');
  if (n) toast(tp(n, '{n} item deleted for good', '{n} items deleted for good'));
  return n > 0;
}

// Library keys, while the grid is shown: Esc drops the selection or the question,
// Ctrl+A selects everything shown, Del deletes the selection.
document.addEventListener('keydown', (e) => {
  if ($('view-library').hidden || e.target.matches('input, select, textarea')) return;
  if (e.key === 'Escape' && (library.confirm || library.selected.size)) clearSelection();
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') selectAll();
  else if (e.key === 'Delete' && library.selected.size) askDeleteSelection();
  else return;
  e.preventDefault();
});

window.openLibrary = async (type) => {
  if (typeof type === 'string') library.filter = { type };
  if ($('view-viewer').hidden) showView('library');
  await loadLibrary();
  if ($('view-viewer').hidden) renderLibrary();
};

for (const button of document.querySelectorAll('#view-library .side-item[data-filter]')) {
  button.onclick = () => setFilter({ type: button.dataset.filter });
}
$('library-sort').onchange = () => { library.sort = $('library-sort').value; renderLibrary(); };
$('library-search').oninput = () => { library.query = $('library-search').value; renderLibrary(); };
$('library-refresh').onclick = async () => {
  await invoke('library_rescan');
  await loadLibrary();
  renderLibrary();
};
$('storage-open').onclick = () => window.openSettings?.('storage');

// Storage used, under the side panel: against the size limit when there is one,
// else against the drive's room.
window.showStorageBar = (storage, limitGb) => {
  const GiB = 1024 ** 3;
  const used = storage.usedBytes;
  const total = limitGb ? limitGb * GiB : (storage.freeBytes != null ? used + storage.freeBytes : 0);
  const share = total ? Math.min(100, (100 * used) / total) : 0;
  $('storage-bar').style.width = `${share}%`;
  $('storage-bar').classList.toggle('full', share >= 95 || storage.pausedForDisk);
  const parts = [limitGb ? t('{used} of {limit} GB', { used: fmtSize(used), limit: fmtNum(limitGb) }) : t('{used} used', { used: fmtSize(used) })];
  if (storage.freeBytes != null) parts.push(t('{free} free', { free: fmtSize(storage.freeBytes) }));
  $('storage-text').textContent = parts.join(' · ');
};

// Viewer

function trackText(n) {
  if (!n) return t('None');
  // D12: track 1 is the mix, 2 the game, 3 the microphone.
  return n === 3 ? t('3 tracks (Mix · Game · Mic)') : tp(n, '{n} track', '{n} tracks');
}

function showDetails(item) {
  const game = gameOf(item);
  const same = library.items.filter((i) => gameKey(i) === gameKey(item)).length;
  $('viewer-game-tile').textContent = initial(game);
  $('viewer-game-name').textContent = game;
  $('viewer-game-count').textContent = t('All {n}', { n: same });
  const d = new Date(item.createdMs);
  $('fact-recorded').textContent = `${fmtDay(item.createdMs)} ${fmtTime(d)}:${two(d.getSeconds())}`;
  const clip = item.kind === 'clip';
  for (const id of ['fact-length', 'fact-audio']) {
    $(id).hidden = !clip;
    $(`${id}-label`).hidden = !clip;
  }
  $('fact-video-label').textContent = clip ? t('Video') : t('Image');
  $('fact-video').textContent = clip ? `${item.width}×${item.height} · ${Math.round(item.fps ?? 0)} fps` : `${item.width}×${item.height}`;
  if (clip) {
    $('fact-length').textContent = fmtSeconds2((item.durationMs ?? 0) / 1000);
    $('fact-audio').textContent = trackText(item.audioTracks);
  }
  $('fact-size').textContent = fmtSize(item.size);
  $('viewer-path').textContent = item.path;
  $('viewer-open-label').textContent = clip ? t('Open in player') : t('Open in image viewer');
  showKept(item.kept);
  // An item in Recently deleted: Restore and Delete now instead of Keep and Delete.
  const gone = item.deletedMs != null;
  $('viewer-keep').hidden = gone;
  $('viewer-restore').hidden = !gone;
  $('viewer-deleted-note').hidden = !gone;
  if (gone) $('viewer-deleted-note').textContent = tp(daysLeft(item), 'In Recently deleted: it goes for good in {n} day.', 'In Recently deleted: it goes for good in {n} days.');
  $('viewer-delete-label').textContent = gone ? t('Delete now') : t('Delete');
}

function showPosition() {
  const list = visible();
  const index = list.findIndex((i) => i.path === library.current?.path);
  $('viewer-position').textContent = index >= 0 ? t('{index} of {total}', { index: index + 1, total: list.length }) : '';
  $('viewer-prev').disabled = index <= 0;
  $('viewer-next').disabled = index < 0 || index >= list.length - 1;
}

// Player (a friends' suggestion, 2026-10-24): Laurel's own bar instead of the
// WebView's controls, whose menu offered to download a file already on the PC:
// play/pause, time, mute, volume and full screen. The trim timeline is the seek
// bar; in full screen the bar has its own. The volume is remembered.
const player = { volume: 1, muted: false, seeking: false };
try {
  const saved = JSON.parse(localStorage.getItem('laurel.player') ?? 'null');
  if (saved) Object.assign(player, { volume: Number(saved.volume ?? 1), muted: !!saved.muted });
} catch {
  // No storage: the defaults stay.
}

function savePlayer() {
  try {
    localStorage.setItem('laurel.player', JSON.stringify({ volume: player.volume, muted: player.muted }));
  } catch {
    // Not remembered this time.
  }
}

function applyVolume() {
  const video = trim.video;
  if (video) {
    video.volume = player.volume;
    video.muted = player.muted;
  }
  $('player-volume').value = player.muted ? 0 : player.volume;
  $('player-mute-icon').setAttribute('d', player.muted ? 'M16 9l5 6M21 9l-5 6' : 'M16.5 9a4 4 0 010 6M19 7a7 7 0 010 10');
  $('player-mute').setAttribute('aria-label', player.muted ? t('Unmute') : t('Mute'));
}

function toggleMute() {
  player.muted = !player.muted;
  if (!player.muted && player.volume === 0) player.volume = 0.5;
  applyVolume();
  savePlayer();
}

$('player-volume').oninput = () => {
  player.volume = Number($('player-volume').value);
  player.muted = player.volume === 0;
  applyVolume();
  savePlayer();
};
$('player-mute').onclick = toggleMute;

// Full screen: the window goes full screen and the stage covers it.
async function setFull(on) {
  if (document.body.classList.contains('player-full') === on) return;
  document.body.classList.toggle('player-full', on);
  $('player-full-icon').setAttribute('d', on ? 'M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5' : 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5');
  $('player-full').setAttribute('aria-label', on ? t('Exit full screen') : t('Full screen'));
  try {
    await appWindow.setFullscreen(on);
  } catch {
    // The stage still covers the window.
  }
}
const toggleFull = () => setFull(!document.body.classList.contains('player-full'));
$('player-full').onclick = toggleFull;

$('player-seek').addEventListener('pointerdown', () => { player.seeking = true; });
$('player-seek').addEventListener('pointerup', () => { player.seeking = false; });
$('player-seek').oninput = () => seek(Number($('player-seek').value));

function openViewer(item) {
  stopPreview();
  releaseMedia();
  library.current = item;
  showView('viewer');
  $('viewer-title').textContent = gameOf(item);
  $('viewer-when').textContent = fmtWhen(item.createdMs);
  $('viewer-status').textContent = '';
  cancelDelete();
  const media = item.kind === 'clip' ? document.createElement('video') : document.createElement('img');
  media.src = convertFileSrc(item.path);
  if (item.kind === 'clip') {
    media.preload = 'auto';
    media.addEventListener('click', () => playSelection());
    media.addEventListener('dblclick', toggleFull);
  } else {
    media.alt = t('Screenshot of {game}', { game: gameOf(item) });
  }
  $('viewer-media').replaceChildren(media);
  // A deleted clip plays but is not trimmed.
  $('trim').hidden = item.kind !== 'clip' || item.deletedMs != null;
  $('player-bar').hidden = item.kind !== 'clip';
  document.querySelector('.stage').classList.add('paused');
  showDetails(item);
  showPosition();
  if (item.kind === 'clip') openTrim(item, media);
}

// Stops playback and lets go of the file (needed before it is deleted).
function releaseMedia() {
  trim.video = null;
  const media = $('viewer-media').firstElementChild;
  if (media instanceof HTMLVideoElement) {
    media.pause();
    media.removeAttribute('src');
    media.load();
  }
  $('viewer-media').replaceChildren();
}

function closeViewer(render = true) {
  if ($('view-viewer').hidden) return;
  const path = library.current?.path;
  setFull(false);
  releaseMedia();
  library.current = null;
  showView('library');
  if (render) {
    renderLibrary();
    const opened = [...document.querySelectorAll('#library-groups .item')].find((c) => c.dataset.path === path);
    opened?.focus({ preventScroll: true });
    opened?.scrollIntoView({ block: 'nearest' });
  }
}

function step(delta) {
  const list = visible();
  const index = list.findIndex((i) => i.path === library.current?.path);
  const next = list[index + delta];
  if (index >= 0 && next) openViewer(next);
}

$('viewer-back').onclick = () => closeViewer();
$('viewer-close').onclick = () => closeViewer();
$('viewer-prev').onclick = () => step(-1);
$('viewer-next').onclick = () => step(1);
$('viewer-game').onclick = () => setFilter({ type: 'game', key: gameKey(library.current), name: gameOf(library.current) });

function showKept(kept) {
  $('viewer-keep').setAttribute('aria-pressed', String(kept));
  $('viewer-keep-label').textContent = kept ? t('Kept|state') : t('Keep');
  $('viewer-keep').title = t('Kept items are never removed by the storage limit');
}

$('viewer-keep').onclick = async () => {
  const item = library.current;
  try {
    const result = await invoke('library_keep', { paths: [item.path], kept: !item.kept });
    if (result.failed.length) throw result.failed[0];
    library.current = result.items[0];
    showKept(library.current.kept);
    $('viewer-status').textContent = library.current.kept ? t('Kept: the storage limit will not remove it.') : '';
  } catch (e) {
    $('viewer-status').textContent = `${e}`;
  }
};

// Delete: to Recently deleted (or, from there, for good), after a confirmation;
// the viewer moves on to the next item.
function askDelete() {
  const item = library.current;
  if (!item) return;
  const clip = item.kind === 'clip';
  if (item.deletedMs != null) {
    $('delete-question').textContent = `${clip ? t('Delete this clip for good?') : t('Delete this screenshot for good?')} ${goneNote()}`;
    $('delete-yes').textContent = t('Delete now');
  } else {
    const question = `${clip ? t('Delete this clip?') : t('Delete this screenshot?')} ${waitNote()}`;
    $('delete-question').textContent = item.kept ? `${question} ${t('It is marked Keep.')}` : question;
    $('delete-yes').textContent = t('Delete');
  }
  $('delete-confirm').hidden = false;
  $('viewer-delete').hidden = true;
  $('delete-yes').focus();
}

function cancelDelete() {
  $('delete-confirm').hidden = true;
  $('viewer-delete').hidden = false;
}

// Runs an action that takes the open item out of its list (delete, restore,
// delete for good), then shows the next item, or the library.
async function leaveCurrent(action) {
  const item = library.current;
  const list = visible();
  const index = list.findIndex((i) => i.path === item.path);
  const next = list[index + 1] ?? list[index - 1] ?? null;
  releaseMedia();
  if (!(await action(item))) {
    openViewer(item);
    return;
  }
  const fresh = next && visible().find((i) => i.path === next.path);
  if (fresh) openViewer(fresh);
  else closeViewer();
}

async function confirmDelete() {
  $('delete-yes').disabled = true;
  await leaveCurrent((item) => (item.deletedMs != null ? purgeItems([item.path]) : deleteItems([item.path])));
  $('delete-yes').disabled = false;
}

$('viewer-delete').onclick = askDelete;
$('viewer-restore').onclick = () => leaveCurrent((item) => restoreItems([item.path]));
$('delete-no').onclick = () => { cancelDelete(); $('viewer-delete').focus(); };
$('delete-yes').onclick = confirmDelete;

// Trim (spec edit.trim): a timeline of frames with two handles; the preview plays
// only the selection, in a loop; I and O set the start and end at the playhead,
// Space plays the selection. The fast trim copies from the keyframe at or before
// the start (shown on the timeline); the exact one re-encodes the video.
// Start and end snap to the clip's frames: the start is the time of the first frame
// kept, the end the end of the last one, so the copy holds exactly the frames the
// preview showed at both marks.

const MIN_TRIM = 0.5;
const trim = { video: null, total: 0, start: 0, end: 0, keyframes: [], frames: [], frame: 1 / 60, token: 0 };

// The frame shown at time t: the last frame that starts at or before it.
function frameAt(t) {
  const f = trim.frames;
  if (!f.length) return { start: t, end: t };
  let lo = 0;
  let hi = f.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (f[mid] <= t + 0.0005) lo = mid;
    else hi = mid - 1;
  }
  return { start: f[lo], end: lo + 1 < f.length ? f[lo + 1] : Math.min(trim.total, f[lo] + trim.frame) };
}

const trimMode = () => document.querySelector('input[name="trim-mode"]:checked').value;
const pct = (t) => `${(100 * t / (trim.total || 1)).toFixed(3)}%`;

// The keyframe the fast trim really starts at.
function fastStart() {
  let at = 0;
  for (const k of trim.keyframes) {
    if (k <= trim.start + 0.001) at = k;
    else break;
  }
  return at;
}

function showTrim() {
  const { start, end } = trim;
  $('timeline-shade-left').style.width = pct(start);
  $('timeline-shade-right').style.left = pct(end);
  $('timeline-selection').style.left = pct(start);
  $('timeline-selection').style.width = pct(end - start);
  $('handle-start').style.left = pct(start);
  $('handle-end').style.left = pct(end);
  for (const [id, value] of [['handle-start', start], ['handle-end', end]]) {
    const handle = $(id);
    handle.setAttribute('aria-valuemin', '0');
    handle.setAttribute('aria-valuemax', trim.total.toFixed(1));
    handle.setAttribute('aria-valuenow', value.toFixed(1));
    handle.setAttribute('aria-valuetext', fmtSeconds(value));
  }
  $('trim-summary').textContent = `${fmtSeconds2(start)} → ${fmtSeconds2(end)} · ${fmtNum(end - start, 2)} s`;
  const exact = trimMode() === 'exact';
  const keyframe = fastStart();
  const earlier = start - keyframe;
  $('timeline-keyframe').hidden = exact || trim.keyframes.length === 0 || earlier < 0.05;
  $('timeline-keyframe').style.left = pct(keyframe);
  if (exact) {
    $('trim-mode-hint').textContent = t('Exact: starts exactly at the start; the video is re-encoded, so it takes longer.');
  } else if (trim.keyframes.length && earlier >= 0.05) {
    $('trim-mode-hint').textContent = t('Fast, no re-encoding: the copy starts at the keyframe at {at} (yellow line), {secs} s before the start.', { at: fmtSeconds(keyframe), secs: fmtNum(earlier, 1) });
  } else {
    $('trim-mode-hint').textContent = t('Fast, no re-encoding: the copy is saved in about a second.');
  }
  $('trim-save').disabled = end - start < MIN_TRIM || (start <= 0.05 && end >= trim.total - 0.05);
}

// Sets the selection from a start moment and an end moment: the start snaps to
// the frame shown there, the end to the end of the frame shown there.
function setTrim(start, end) {
  trim.start = frameAt(Math.max(0, Math.min(start, trim.total - MIN_TRIM))).start;
  trim.end = frameAt(Math.min(trim.total, Math.max(end, trim.start + MIN_TRIM)) - 0.0005).end;
  if (end >= trim.total - 0.0005) trim.end = trim.total;
  showTrim();
}

async function openTrim(item, video) {
  const token = ++trim.token;
  trim.video = video;
  applyVolume();
  trim.total = item.durationMs / 1000;
  trim.keyframes = [];
  trim.frames = [];
  $('timeline-strip').replaceChildren();
  setTrim(0, trim.total);
  video.addEventListener('loadedmetadata', () => {
    if (trim.video === video && Number.isFinite(video.duration)) {
      trim.total = video.duration;
      setTrim(trim.start, Math.min(trim.end, trim.total) || trim.total);
    }
  });
  requestAnimationFrame(() => followPlayhead(video));
  try {
    const info = await invoke('library_trim_info', { path: item.path });
    if (token !== trim.token) return;
    trim.keyframes = info.keyframes;
    trim.frames = info.frameTimes ?? [];
    if (trim.frames.length > 1) trim.frame = trim.frames[1] - trim.frames[0];
    setTrim(trim.start, trim.end);
    $('timeline-strip').replaceChildren(...info.frames.map((path) => {
      const img = document.createElement('img');
      img.src = convertFileSrc(path);
      img.alt = '';
      return img;
    }));
    showTrim();
  } catch (e) {
    if (token === trim.token) $('viewer-status').textContent = t('Timeline unavailable: {error}', { error: e });
  }
}

// Keeps the playhead on the video and the playback inside the selection.
function followPlayhead(video) {
  if (trim.video !== video) return;
  if (!video.paused && (video.currentTime >= trim.end || video.currentTime < trim.start - 0.25)) video.currentTime = trim.start;
  const now = Math.min(video.currentTime, trim.total);
  $('timeline-playhead').style.left = pct(now);
  $('trim-play').firstChild.textContent = `${video.paused ? t('Play the selection') : t('Pause')} `;
  $('player-time').textContent = `${fmtSeconds(now)} / ${fmtSeconds(trim.total)}`;
  $('player-play-icon').setAttribute('d', video.paused ? 'M7 5l12 7-12 7z' : 'M7 5h4v14H7zM13 5h4v14h-4z');
  $('player-play').setAttribute('aria-label', video.paused ? t('Play') : t('Pause'));
  document.querySelector('.stage').classList.toggle('paused', video.paused);
  if (!player.seeking) {
    $('player-seek').max = trim.total || 1;
    $('player-seek').value = now;
  }
  requestAnimationFrame(() => followPlayhead(video));
}

// Shows the frame at t; a millisecond into the frame, so rounding cannot show the
// previous one.
function seek(t) {
  if (trim.video) trim.video.currentTime = Math.max(0, Math.min(t + 0.001, trim.total));
}

function timeAt(clientX) {
  const box = $('timeline').getBoundingClientRect();
  return Math.max(0, Math.min(1, (clientX - box.left) / box.width)) * trim.total;
}

// Dragging a handle moves that edge and shows its frame; a click elsewhere seeks.
function dragHandle(handle, edge) {
  handle.addEventListener('pointerdown', (e) => {
    e.stopPropagation();
    handle.setPointerCapture(e.pointerId);
    trim.video?.pause();
    const move = (ev) => {
      const t = timeAt(ev.clientX);
      if (edge === 'start') setTrim(Math.min(t, trim.end - MIN_TRIM), trim.end);
      else setTrim(trim.start, Math.max(t, trim.start + MIN_TRIM));
      seek(edge === 'start' ? trim.start : trim.end - trim.frame);
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
  });
  handle.addEventListener('keydown', (e) => {
    const step = { ArrowLeft: -0.1, ArrowRight: 0.1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const d = step * (e.shiftKey ? 10 : 1);
    if (edge === 'start') setTrim(Math.min(trim.start + d, trim.end - MIN_TRIM), trim.end);
    else setTrim(trim.start, Math.max(trim.end + d, trim.start + MIN_TRIM));
    seek(edge === 'start' ? trim.start : trim.end - trim.frame);
  });
}
dragHandle($('handle-start'), 'start');
dragHandle($('handle-end'), 'end');
// A click on the timeline moves the video there; dragging keeps moving it.
$('timeline').addEventListener('pointerdown', (e) => {
  const video = trim.video;
  if (!video) return;
  const timeline = $('timeline');
  timeline.setPointerCapture(e.pointerId);
  const wasPlaying = !video.paused;
  video.pause();
  seek(timeAt(e.clientX));
  const move = (ev) => seek(timeAt(ev.clientX));
  const up = () => {
    timeline.removeEventListener('pointermove', move);
    timeline.removeEventListener('pointerup', up);
    timeline.removeEventListener('pointercancel', up);
    if (wasPlaying) playSelection();
  };
  timeline.addEventListener('pointermove', move);
  timeline.addEventListener('pointerup', up);
  timeline.addEventListener('pointercancel', up);
});

function setStartHere() {
  if (trim.video) setTrim(Math.min(trim.video.currentTime, trim.end - MIN_TRIM), trim.end);
}
function setEndHere() {
  if (trim.video) setTrim(trim.start, Math.max(trim.video.currentTime, trim.start + MIN_TRIM));
}
function playSelection() {
  const video = trim.video;
  if (!video) return;
  if (!video.paused) {
    video.pause();
    return;
  }
  if (video.currentTime < trim.start || video.currentTime >= trim.end - 0.05) video.currentTime = trim.start;
  video.play();
}

$('trim-set-start').onclick = setStartHere;
$('trim-set-end').onclick = setEndHere;
$('trim-play').onclick = playSelection;
$('player-play').onclick = playSelection;
for (const radio of document.querySelectorAll('input[name="trim-mode"]')) radio.onchange = showTrim;

// Viewer keys, in the capture phase so they work wherever the focus is: ← →
// previous and next, Del delete, Esc out of full screen or back to the library, M
// mute, F full screen, and I, O and Space for the trim.
document.addEventListener('keydown', (e) => {
  if ($('view-viewer').hidden || e.ctrlKey || e.altKey || e.metaKey) return;
  if (e.target.matches('input[type="text"], input[type="number"], input[type="search"], select')) return;
  const key = e.key;
  // A clip that plays (deleted ones too), and one that can be trimmed.
  const playing = trim.video != null;
  const trimming = !$('trim').hidden;
  if (key === 'Escape') {
    if (document.body.classList.contains('player-full')) setFull(false);
    else if (!$('delete-confirm').hidden) cancelDelete();
    else closeViewer();
  } else if (playing && key.toLowerCase() === 'm') {
    toggleMute();
  } else if (playing && key.toLowerCase() === 'f') {
    toggleFull();
  } else if (key === 'Delete') {
    askDelete();
  } else if ((key === 'ArrowLeft' || key === 'ArrowRight') && !e.target.matches('.timeline-handle, input[type="range"]')) {
    step(key === 'ArrowLeft' ? -1 : 1);
  } else if (trimming && key.toLowerCase() === 'i') {
    setStartHere();
  } else if (trimming && key.toLowerCase() === 'o') {
    setEndHere();
  } else if (playing && key === ' ' && !e.target.matches('button')) {
    // Space on a focused button presses it; anywhere else, the video included, it
    // plays the selection instead of the whole clip.
    playSelection();
  } else {
    return;
  }
  e.preventDefault();
  e.stopPropagation();
}, true);

$('trim-save').onclick = async () => {
  const item = library.current;
  const exact = trimMode() === 'exact';
  $('trim-save').disabled = true;
  $('viewer-status').textContent = exact ? t('Re-encoding the trimmed copy…') : t('Saving the trimmed copy…');
  try {
    const copy = await invoke('library_trim', { path: item.path, start: trim.start, end: trim.end, exact });
    $('viewer-status').textContent = t('Saved {file} ({length})', { file: fileName(copy.path), length: fmtDuration(copy.durationMs) });
    activity(t('Trimmed copy saved: {file}', { file: fileName(copy.path) }), 'clip');
  } catch (e) {
    $('viewer-status').textContent = t('Trim failed: {error}', { error: e });
  } finally {
    showTrim();
  }
};

const SHARE_DONE = { 'copy-file': t('File copied. Paste it in a chat or a folder.'), 'copy-path': t('Path copied.'), show: '', open: '' };
for (const button of document.querySelectorAll('[data-share]')) {
  button.onclick = async () => {
    try {
      await invoke('share_action', { action: button.dataset.share, path: library.current.path });
      $('viewer-status').textContent = SHARE_DONE[button.dataset.share];
    } catch (e) {
      $('viewer-status').textContent = `${e}`;
    }
  };
}

// New, trimmed, kept or removed items: the grid follows; an open viewer keeps its
// video and only refreshes the details and the position.
listen('library-changed', async () => {
  await loadLibrary();
  if (!$('view-viewer').hidden && library.current) {
    const fresh = [...library.items, ...library.deleted].find((i) => i.path === library.current.path);
    if (fresh) {
      library.current = fresh;
      showDetails(fresh);
    }
    showPosition();
  } else {
    renderLibrary();
  }
});

