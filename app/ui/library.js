// Library, the home screen (specs library.browse, library.thumbnails, edit.trim,
// share.local, library.trash, library.tags): a side panel with All, Clips,
// Screenshots, Kept, Recently deleted, the games, the tags and the storage used; a
// grid of clips and screenshots grouped by day, sorted and searched (by game, file
// name, achievement or tag), with a muted preview of a clip under the
// pointer, where several items can be selected and kept, deleted or restored at
// once; and the viewer, with Laurel's own player, the previous and next item, the
// trim under the video and the details and actions on the right, Delete included.
// Deleted items wait in Recently deleted for the days chosen in Storage, and Undo
// brings them back right after a delete.
const { convertFileSrc } = window.__TAURI__.core;

// An item's file for the WebView, with its size and length: a trim saved in its
// original's place keeps the path, and the WebView would play the cached original.
const mediaSrc = (item) => `${convertFileSrc(item.path)}?v=${item.size}-${item.durationMs ?? 0}`;

// tagCounts: each tag in use, by ID, with its items outside Recently deleted and in
// it, as the index counts them ({ funny: { items: 2, deleted: 1 } }).
const library = { items: [], deleted: [], tagCounts: {}, filter: { type: 'all' }, sort: 'new', query: '', current: null, selected: new Set(), anchor: null, confirm: null };
// The open tag picker (see Tags below): what it works on (target, viewerTags or
// barTags, null while it is closed), the button that opened it, and the box that
// holds both.
const tagPicker = { target: null, button: null, box: null };

const typeTitle = (type) => ({ all: t('All'), clip: t('Clips'), screenshot: t('Screenshots'), kept: t('Kept'), achievement: t('Achievements'), deleted: t('Recently deleted') })[type];
// One icon for every tag in the side panel: the tags have no icon of their own.
const TAG_ICON = '<path d="M4 4h7l9 9-7 7-9-9z"/><circle cx="8" cy="8" r="1.4"/>';
const TROPHY = '<path d="M8 4h8v5a4 4 0 01-8 0V4z"/><path d="M8 6H5.5a2.5 2.5 0 002.7 3.5M16 6h2.5a2.5 2.5 0 01-2.7 3.5"/><path d="M12 13v4M8.5 20h7M10 17h4"/>';
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

// "Hi Mom! · 2.1% of players", the achievement an automatic clip was saved for.
function achievementText(a) {
  const share = a.percent != null ? ` · ${t('{percent}% of players', { percent: fmtNum(a.percent, 1) })}` : '';
  const more = a.others?.length ? ` · ${tp(a.others.length, '+{n} more', '+{n} more')}` : '';
  return `${t('Achievement: {name}', { name: a.name || a.id })}${share}${more}`;
}

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
  if (filter.type === 'achievement') return item.achievement != null;
  if (filter.type === 'game') return gameKey(item) === filter.key;
  if (filter.type === 'tag') return (item.tags ?? []).includes(filter.id);
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
  const tagQuery = foldName(query);
  const source = inTrash() ? library.deleted : library.items;
  const items = source.filter((item) => (inTrash() || matches(item, library.filter))
    && (!query || gameOf(item).toLowerCase().includes(query) || fileName(item.path).toLowerCase().includes(query)
      || (item.achievement?.name ?? '').toLowerCase().includes(query)
      || (tagQuery && (item.tags ?? []).some((id) => tagFound(id, tagQuery)))));
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
  const counts = { all: library.items.length, clip: 0, screenshot: 0, kept: 0, achievement: 0, deleted: library.deleted.length };
  for (const item of library.items) {
    counts[item.kind] += 1;
    if (item.kept) counts.kept += 1;
    if (item.achievement) counts.achievement += 1;
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
  // The tags with items outside Recently deleted, by the name shown.
  const tags = Object.entries(library.tagCounts).filter(([, count]) => count.items > 0)
    .map(([id, count]) => ({ id, name: tagName(id), count: count.items }))
    .sort((a, b) => a.name.localeCompare(b.name, LOCALE) || a.id.localeCompare(b.id));
  $('side-tags-label').hidden = !tags.length;
  $('side-tags').replaceChildren(...tags.map((tag) => {
    const button = document.createElement('button');
    button.className = 'side-item';
    button.classList.toggle('active', library.filter.type === 'tag' && library.filter.id === tag.id);
    const name = document.createElement('span');
    name.className = 'side-name';
    name.textContent = tag.name;
    name.title = tag.name;
    const count = document.createElement('span');
    count.className = 'count';
    count.textContent = tag.count;
    button.append(svgIcon(TAG_ICON), name, count);
    button.onclick = () => setFilter({ type: 'tag', id: tag.id });
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
    video.src = mediaSrc(item);
    card.querySelector('.thumb').append(video);
    card.classList.add('previewing');
  }, PREVIEW_DELAY);
}

const CHECK = '<path d="M5 12l4 4 10-10" stroke-width="3"/>';

// The tags on a card (spec library.tags, Cartão; the owner's point: a long name
// never leaks from its chip): the first two in chip order and "+N" for the rest,
// all of them in the tooltip. The CSS shares the row: each chip grows up to its own
// width, so a short name stays whole while a long one is cut with "…", and "+N"
// never shrinks. Only "a second chip left with fewer than about four characters
// goes into +N" needs a measure, in fitCardTags.
const CARD_TAGS_SHOWN = 2;
const CARD_MIN_VISIBLE = 4;

function cardTags(tags) {
  const row = document.createElement('span');
  row.className = 'card-tags';
  row.dataset.count = tags.length;
  const names = tags.map(tagName);
  const all = names.join('\n');
  for (const name of names.slice(0, CARD_TAGS_SHOWN)) {
    const chip = document.createElement('span');
    chip.className = 'card-tag';
    chip.textContent = name;
    chip.title = all;
    row.append(chip);
  }
  const more = document.createElement('span');
  more.className = 'card-more';
  more.title = all;
  row.append(more);
  showCardTags(row, CARD_TAGS_SHOWN);
  return row;
}

// Shows the first `shown` chips of a row and "+N" for the others.
function showCardTags(row, shown) {
  const count = Number(row.dataset.count);
  const chips = row.querySelectorAll('.card-tag');
  chips.forEach((chip, i) => { chip.hidden = i >= shown; });
  const more = row.lastElementChild;
  more.hidden = count <= shown;
  more.textContent = `+${count - shown}`;
}

// Moves into "+N" each second chip that shows fewer than CARD_MIN_VISIBLE
// characters, as the user sees them (graphemes: "👍🏽" is one): its first ones and
// the "…" do not fit in it, and its whole name does not either. On each render
// and when the grid's width changes; the writes go first and the reads after, so
// the whole grid is laid out once.
const textMeasurer = document.createElement('canvas').getContext('2d');
const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' });

function fitCardTags() {
  const rows = [...$('library-groups').querySelectorAll('.card-tags')].filter((row) => Number(row.dataset.count) > 1);
  for (const row of rows) showCardTags(row, CARD_TAGS_SHOWN);
  // Hidden (the viewer or another section open): the width coming back fits them.
  if (!rows.length || !rows[0].getBoundingClientRect().width) return;
  const style = getComputedStyle(rows[0].querySelector('.card-tag'));
  textMeasurer.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
  const width = (text) => textMeasurer.measureText(text).width;
  const squeezed = rows.filter((row) => {
    const chip = row.querySelectorAll('.card-tag')[1];
    const room = chip.getBoundingClientRect().width - padding;
    const name = chip.textContent;
    const first = [...graphemes.segment(name)].slice(0, CARD_MIN_VISIBLE).map((g) => g.segment).join('');
    // A whole name within a pixel of its room fits: the canvas and the layout can
    // measure the same text a fraction of a pixel apart.
    return room + 1 < width(name) && room < width(`${first}…`);
  });
  for (const row of squeezed) showCardTags(row, 1);
}

let gridWidth = 0;
new ResizeObserver(([entry]) => {
  const width = Math.round(entry.contentRect.width);
  if (width === gridWidth) return;
  gridWidth = width;
  if (width) fitCardTags();
}).observe($('library-groups'));
// A web font that ends loading changes the names' widths.
document.fonts.addEventListener('loadingdone', () => fitCardTags());

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
  const marks = document.createElement('span');
  marks.className = 'marks';
  if (item.kept) {
    const kept = document.createElement('span');
    kept.className = 'kept';
    kept.textContent = `★ ${t('Kept|state')}`;
    kept.title = t('Kept: never removed by the storage limit');
    marks.append(kept);
  }
  if (item.achievement) {
    const trophy = document.createElement('span');
    trophy.className = 'trophy';
    trophy.append(svgIcon(TROPHY, 13));
    trophy.title = achievementText(item.achievement);
    trophy.setAttribute('aria-label', trophy.title);
    marks.append(trophy);
  }
  if (marks.childElementCount) thumb.append(marks);
  // The bottom of the thumbnail: on the left the tags, over the days left in
  // Recently deleted; on the right a clip's length, which the tags stop before.
  const foot = document.createElement('span');
  foot.className = 'thumb-foot';
  const left = document.createElement('span');
  left.className = 'thumb-left';
  if (item.tags?.length) left.append(cardTags(item.tags));
  if (item.deletedMs != null) {
    const days = document.createElement('span');
    days.className = 'deleted-badge';
    days.textContent = tp(daysLeft(item), 'Goes in {n} day', 'Goes in {n} days');
    left.append(days);
  }
  if (left.childElementCount) foot.append(left);
  if (item.kind === 'clip') {
    const play = document.createElement('span');
    play.className = 'play';
    const icon = svgIcon('<path d="M8 5l12 7-12 7z" fill="currentColor" stroke="none"/>', 14);
    play.append(icon);
    const badge = document.createElement('span');
    badge.className = 'badge';
    badge.textContent = fmtDuration(item.durationMs);
    thumb.append(play);
    foot.append(badge);
  }
  if (foot.childElementCount) thumb.append(foot);
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
    achievement: t('No achievement clips yet. With Settings, Auto-clip on, Laurel saves a clip when you unlock a rare Steam achievement.'),
    deleted: tp(deletedDays(), 'Nothing here. Deleted clips and screenshots wait here for {n} day before they go for good.', 'Nothing here. Deleted clips and screenshots wait here for {n} days before they go for good.'),
  }[library.filter.type] ?? t('No clips yet. Press {key} in a game to save the last seconds.', { key: clipKey });
}

// The grid's title: the chosen game's or tag's name, else the filter's.
function filterTitle(filter) {
  if (filter.type === 'game') return filter.name;
  if (filter.type === 'tag') return tagName(filter.id);
  return typeTitle(filter.type);
}

function renderLibrary() {
  stopPreview();
  renderSide();
  const items = visible();
  // The selection keeps only what is shown.
  const shown = new Set(items.map((i) => i.path));
  library.selected = new Set([...library.selected].filter((path) => shown.has(path)));
  $('library-title').textContent = filterTitle(library.filter);
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
  fitCardTags();
  showSelection();
}

async function loadLibrary() {
  try {
    [library.items, library.deleted, library.tagCounts] = await Promise.all([invoke('library_list', {}), invoke('library_list', { deleted: true }), invoke('library_tag_counts')]);
  } catch (e) {
    library.items = [];
    library.deleted = [];
    library.tagCounts = {};
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

// A custom tag deleted in Settings › Tags: a library filtered by it goes back to
// All, as no item can have it now and its name is gone.
function leaveTagFilter(id) {
  if (library.filter.type === 'tag' && library.filter.id === id) setFilter({ type: 'all' });
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
  // The bar's tag picker closes when the selection it serves changes.
  if (tagPicker.target === barTags && !barPickerServes(barTags.paths)) closeTagPicker(false);
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
  for (const id of ['sel-keep', 'sel-tags-box', 'sel-delete']) $(id).hidden = inTrash();
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
$('sel-tags').onclick = () => ($('tag-picker').hidden ? openBarTagPicker() : closeTagPicker());
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
  if (result.failed.length) {
    const error = t('Could not delete: {error}', { error: result.failed.join('; ') });
    activity(error, 'bad');
    if (!n) toast(error);
  }
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
// Ctrl+A selects everything shown, Del deletes the selection. Keys in the open tag
// picker are its own.
document.addEventListener('keydown', (e) => {
  if ($('view-library').hidden || e.target.matches('input, select, textarea') || e.target.closest('#tag-picker')) return;
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
  const a = item.achievement;
  $('fact-achievement').hidden = $('fact-achievement-label').hidden = !a;
  if (a) {
    const share = a.percent != null ? ` · ${t('{percent}% of players', { percent: fmtNum(a.percent, 1) })}` : '';
    const more = a.others?.length ? ` · ${a.others.join(', ')}` : '';
    $('fact-achievement').textContent = `${a.name || a.id}${share}${more}`;
    $('fact-achievement').title = a.description ?? '';
  }
  $('viewer-path').textContent = item.path;
  $('viewer-open-label').textContent = clip ? t('Open in player') : t('Open in image viewer');
  showKept(item.kept);
  showTags(item);
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
  stretchEnd(on ? 'full screen on' : 'full screen off');
  document.body.classList.toggle('player-full', on);
  $('player-full-icon').setAttribute('d', on ? 'M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5' : 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5');
  $('player-full').setAttribute('aria-label', on ? t('Exit full screen') : t('Full screen'));
  try {
    await appWindow.setFullscreen(on);
  } catch {
    // The stage still covers the window.
  }
  if (trim.video && !trim.video.paused) stretchStart(trim.video);
}
const toggleFull = () => setFull(!document.body.classList.contains('player-full'));
$('player-full').onclick = toggleFull;

$('player-seek').addEventListener('pointerdown', () => { player.seeking = true; });
$('player-seek').addEventListener('pointerup', () => { player.seeking = false; });
$('player-seek').oninput = () => seek(Number($('player-seek').value));

// Player diagnostics (A30: a friend's clips looked slow motion after he moved the
// window to his 60 Hz second monitor). Each stretch of playback of 2 s or more
// ends in one app.log line (player_stats in main.rs): the monitor, media seconds
// per wall second, decoded and dropped frames and how often the page was drawn. A
// stretch ends at a pause, when the viewer closes or opens another item, when the
// window goes to another monitor, in or out of full screen, or is hidden. Per
// frame it costs a counter and a subtraction in followPlayhead.
const MIN_STRETCH_S = 2;
let stretch = null;
const screenKey = () => `${screen.availLeft},${screen.availTop},${screen.width}x${screen.height},${devicePixelRatio}`;

function stretchStart(video) {
  if (stretch || video.paused || document.hidden || trim.video !== video) return;
  const q = video.getVideoPlaybackQuality();
  const now = performance.now();
  stretch = {
    video, item: library.current, wall: now, checked: now, screen: screenKey(),
    media: 0, last: video.currentTime, skip: false, frames: 0,
    decoded: q.totalVideoFrames, dropped: q.droppedVideoFrames,
    full: document.body.classList.contains('player-full'),
  };
  invoke('player_stats', { start: true }).catch(() => {});
}

function stretchEnd(ended) {
  const s = stretch;
  stretch = null;
  if (!s?.item) return;
  const wall = (performance.now() - s.wall) / 1000;
  if (wall < MIN_STRETCH_S) return;
  const q = s.video.getVideoPlaybackQuality();
  const stats = {
    file: s.item.path.split(/[\\/]/).pop(), width: s.video.videoWidth, height: s.video.videoHeight,
    fps: s.item.fps ?? null, wall, media: s.media, frames: s.frames, full: s.full,
    decoded: q.totalVideoFrames - s.decoded, dropped: q.droppedVideoFrames - s.dropped,
    rate: s.video.playbackRate, ended,
  };
  invoke('player_stats', { start: false, stats }).catch(() => {});
}

// Called by followPlayhead on each frame it draws.
function stretchFrame(video) {
  const s = stretch;
  if (s?.video !== video) return;
  s.frames++;
  const at = video.currentTime;
  // Forward steps count; a seek or the selection's loop does not.
  if (!s.skip && at > s.last && at - s.last < 1) s.media += at - s.last;
  s.skip = false;
  s.last = at;
  const now = performance.now();
  if (now - s.checked >= 500) {
    s.checked = now;
    if (screenKey() !== s.screen) {
      stretchEnd('moved to another monitor');
      stretchStart(video);
    }
  }
}

document.addEventListener('visibilitychange', () => {
  if (document.hidden) stretchEnd('window hidden');
  else if (trim.video && !trim.video.paused) stretchStart(trim.video);
});

function openViewer(item) {
  stopPreview();
  releaseMedia();
  library.current = item;
  showView('viewer');
  $('viewer-title').textContent = gameOf(item);
  $('viewer-when').textContent = fmtWhen(item.createdMs);
  $('viewer-status').textContent = '';
  cancelDelete();
  closeTagPicker(false);
  const media = item.kind === 'clip' ? document.createElement('video') : document.createElement('img');
  media.src = mediaSrc(item);
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
  stretchEnd('viewer closed or another item opened');
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
  closeTagPicker(false);
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

// Tags (spec library.tags): the item's tags as chips under its game, in the order
// the index gives them (the standard list's, then the custom tags by name); ×
// takes one off, and "+ Tag" or T opens the picker, where a click puts a tag on or
// takes it off at once. The search finds a tag by its name in the interface's
// language or in English, ignoring case, accents and spaces, and a name no tag
// has offers "Create": Enter or a click creates the custom tag and puts it on the
// item. In Recently deleted the tags are read-only. The selection bar's Tags button
// opens the same picker for the selected items (not in Recently deleted): each tag
// shows ✓ when every one has it, – when some do and nothing when none does; a click
// on – or nothing puts it on every one, on ✓ takes it off every one (the index
// decides, library_tag_click), and "Create" puts the new tag on every one.
const tagCatalog = { groups: [], tags: [] };
const loadTagCatalog = async () => Object.assign(tagCatalog, await invoke('library_tags'));
loadTagCatalog().then(() => {
  if (library.current) showTags(library.current);
  // The side panel, the title and the search read the names; with no tag in use
  // (or the library not read yet) nothing shows one.
  else if (!$('view-library').hidden && Object.keys(library.tagCounts).length) renderLibrary();
}).catch((e) => activity(t('Tags unavailable: {error}', { error: e }), 'bad'));

// The one place a tag's ID becomes the name the interface shows.
const tagName = (id) => tagCatalog.tags.find((tag) => tag.id === id)?.name ?? id;
// A name as the index compares two (app/core/src/library/tags.rs, fold and
// check_name), step by step: lowercase, decomposed, without marks, and without the
// spaces Rust's trim and is_whitespace know (Unicode's White_Space, which `\s` and
// trim() are not), so that "Create" shows for the names the index takes but long ones.
const foldName = (s) => s.toLowerCase().normalize('NFD').replace(/[\p{M}\p{White_Space}]/gu, '');
const trimName = (s) => s.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '');
// A tag's names the picker's search compares, folded: a custom tag has no English one.
const foldedNames = (tag) => [tag.name, tag.english].filter(Boolean).map(foldName);
// The library search finds an item by a tag's name in the interface's language or
// in English, by the picker's rule; `query` is already folded.
const tagFound = (id, query) => foldedNames(tagCatalog.tags.find((tag) => tag.id === id) ?? { name: id }).some((name) => name.includes(query));

// A chip's element with a tag's name, cut with "…" when it does not fit; the full
// name shows on hover.
function tagChipOf(tag, name) {
  const chip = document.createElement(tag);
  chip.className = 'tag-chip';
  chip.title = name;
  const text = document.createElement('span');
  text.className = 'tag-name';
  text.textContent = name;
  chip.append(text);
  return chip;
}

function tagChip(id, removable) {
  const chip = tagChipOf('span', tagName(id));
  if (removable) {
    const remove = document.createElement('button');
    remove.className = 'tag-remove';
    remove.append(svgIcon('<path d="M6 6l12 12M18 6L6 18"/>', 12));
    remove.title = t('Take off {tag}', { tag: tagName(id) });
    remove.setAttribute('aria-label', remove.title);
    remove.onclick = () => takeOffTag(id);
    chip.append(remove);
  }
  return chip;
}

function showTags(item) {
  const gone = item.deletedMs != null;
  const tags = item.tags ?? [];
  const add = $('viewer-tag-add');
  const focused = document.activeElement;
  $('viewer-tags').hidden = gone && !tags.length;
  add.hidden = gone;
  $('key-hint-tags').hidden = gone;
  $('viewer-tag-list').replaceChildren(...tags.map((id) => tagChip(id, !gone)), add);
  if (focused === add || (focused?.classList.contains('tag-remove') && !focused.isConnected)) add.focus();
  if (gone) closeTagPicker(false);
  showTagStates();
}

// A tag's state in the open picker: 'all' (✓), 'some' (–) or 'none'.
const tagState = (id) => tagPicker.target?.state(id) ?? 'none';
const PRESSED = { all: 'true', some: 'mixed', none: 'false' };
const showTagStates = () => {
  for (const option of $('tag-options').querySelectorAll('[data-tag]')) option.setAttribute('aria-pressed', PRESSED[tagState(option.dataset.tag)]);
};

// Fills the picker with the groups and tags the search finds, and offers to create
// a tag with the name typed when no tag has that name (the index's rule, which
// ignores case, accents and spaces, so "engracado" or "funny" finds "Engraçado").
function showTagOptions() {
  const typed = trimName($('tag-search').value);
  const query = foldName(typed);
  const groups = [];
  for (const group of tagCatalog.groups) {
    const found = tagCatalog.tags.filter((tag) => tag.group === group.id && foldedNames(tag).some((name) => name.includes(query)));
    if (!found.length) continue;
    const label = document.createElement('div');
    label.className = 'tag-group-label';
    label.textContent = group.name;
    const list = document.createElement('div');
    list.className = 'tag-chips';
    for (const tag of found) {
      const option = tagChipOf('button', tag.name);
      option.classList.add('tag-option');
      option.dataset.tag = tag.id;
      option.setAttribute('aria-pressed', PRESSED[tagState(tag.id)]);
      const check = svgIcon('<path d="M5 12l4 4 10-10"/>', 12);
      check.classList.add('tag-check');
      const some = svgIcon('<path d="M6 12h12"/>', 12);
      some.classList.add('tag-some');
      option.prepend(check, some);
      option.onclick = () => toggleTag(tag.id);
      list.append(option);
    }
    const box = document.createElement('div');
    box.className = 'tag-group';
    box.append(label, list);
    groups.push(box);
  }
  $('tag-options').replaceChildren(...groups);
  const create = $('tag-create');
  create.hidden = !typed || tagCatalog.tags.some((tag) => foldedNames(tag).includes(query));
  $('tag-create-label').textContent = t('Create “{name}”', { name: typed });
  create.title = $('tag-create-label').textContent;
  $('tag-none').hidden = groups.length > 0 || !create.hidden;
}

// Opens the picker on `target`, in `box`, under the `button` that opened it.
function showPicker(target, button, box) {
  Object.assign(tagPicker, { target, button, box });
  box.append($('tag-picker'));
  $('tag-picker').hidden = false;
  button.setAttribute('aria-expanded', 'true');
  $('tag-search').value = '';
  $('tag-error').hidden = true;
  showTagOptions();
  $('tag-search').focus();
}

function openTagPicker() {
  const item = library.current;
  if (!item || item.deletedMs != null) return;
  showPicker(viewerTags, $('viewer-tag-add'), $('viewer-tags'));
}

// Whether the selection bar's picker serves `paths`: the items selected in the
// library on screen, no more and no fewer, outside Recently deleted and with no
// question in the bar.
function barPickerServes(paths) {
  if ($('view-library').hidden || library.confirm || inTrash() || !paths.length) return false;
  return paths.length === library.selected.size && paths.every((path) => library.selected.has(path));
}

// The selection bar's picker, with each tag's state over the selected items.
async function openBarTagPicker() {
  const paths = [...library.selected];
  if (!barPickerServes(paths)) return;
  let states;
  try {
    // After the changes still queued, from a picker closed right after a click.
    await tagChanges;
    states = await invoke('library_tag_states', { paths });
  } catch (e) {
    toast(t('Tags unavailable: {error}', { error: e }));
    return;
  }
  // The selection or the view may have changed while the index counted.
  if (!$('tag-picker').hidden || !barPickerServes(paths)) return;
  Object.assign(barTags, { paths, states });
  showPicker(barTags, $('sel-tags'), $('sel-tags-box'));
}

function closeTagPicker(focus = true) {
  if ($('tag-picker').hidden) return;
  $('tag-picker').hidden = true;
  tagPicker.target = null;
  tagPicker.button.setAttribute('aria-expanded', 'false');
  if (focus && !tagPicker.button.hidden) tagPicker.button.focus();
}

const showTagError = (e) => {
  $('tag-error').textContent = `${e}`;
  $('tag-error').hidden = false;
};

// One change at a time, in the order of the clicks: each reads the tags the one
// before left, so two quick clicks on a tag put it on and take it off again.
// `tagWrites` counts the changes that are waiting or done, so that an item read
// meanwhile does not bring back the tags of before (see library-changed).
let tagChanges = Promise.resolve();
const tagWrites = { waiting: 0, done: 0 };
// Queues `change`, an async tag change; a failure goes to `failed`.
function queueTagChange(change, failed) {
  tagWrites.waiting += 1;
  tagChanges = tagChanges.then(change).catch(failed).finally(() => {
    tagWrites.waiting -= 1;
    tagWrites.done += 1;
  });
}
// Queues `write(item)`, which gives the open item's tag change as library_tag
// does, and shows the item it leaves.
function writeTags(write, failed) {
  const path = library.current?.path;
  queueTagChange(async () => {
    const item = library.current;
    if (!item || item.path !== path || item.deletedMs != null) return;
    const result = await write(item);
    if (result.failed.length) throw result.failed[0];
    if (library.current?.path !== path) return;
    library.current = result.items[0];
    showTags(library.current);
  }, failed);
}
function changeTag(id, wanted) {
  writeTags((item) => invoke('library_tag', { paths: [item.path], tag: id, on: wanted(item.tags ?? []) }), (e) => {
    $('viewer-status').textContent = `${e}`;
  });
}
const takeOffTag = (id) => changeTag(id, () => false);
// Queues `write(paths)`, a change to the tags of the items the bar's picker opened
// for, which gives the items as library_tag does and the states it leaves; the
// picker shows them while it serves those items.
function writeBarTags(write, failed) {
  const paths = barTags.paths;
  queueTagChange(async () => {
    const result = await write(paths);
    if (tagPicker.target === barTags && barTags.paths === paths) {
      barTags.states = result.states;
      showTagStates();
    }
    if (result.failed.length) throw result.failed.join('; ');
  }, failed);
}

// What the picker works on: the item open in the viewer, or the items selected
// when the selection bar's picker opened (paths), with each tag's state over them
// as the index gave it (states: { kill: 'some', ... }). Each gives a tag's state,
// does a click on a tag and creates a custom tag with the name typed, which goes on
// its item or items; `refused` gets why the index refused the name.
const viewerTags = {
  state: (id) => ((library.current?.tags ?? []).includes(id) ? 'all' : 'none'),
  toggle: (id) => changeTag(id, (tags) => !tags.includes(id)),
  create: (name, refused) => writeTags(async (item) => {
    const result = await invoke('library_tag_create', { name, paths: [item.path] });
    await loadTagCatalog();
    showTagOptions();
    return result;
  }, refused),
};
const barTags = {
  paths: [],
  states: {},
  state: (id) => barTags.states[id] ?? 'none',
  // The index decides from the states it has then (library_tag_click).
  toggle: (id) => writeBarTags((paths) => invoke('library_tag_click', { paths, tag: id }), showTagError),
  create: (name, refused) => writeBarTags(async (paths) => {
    const result = await invoke('library_tag_create', { name, paths });
    await loadTagCatalog();
    const states = await invoke('library_tag_states', { paths });
    // With the old states for a moment: writeBarTags shows the new ones in the same task.
    showTagOptions();
    return { ...result, states };
  }, refused),
};
const toggleTag = (id) => tagPicker.target?.toggle(id);

// Creates a custom tag with the name typed in the search and puts it on what the
// picker works on. The search empties at once, so a second Enter cannot ask for
// the same tag; when the index refuses the name, it comes back with the reason.
function createTag() {
  const name = $('tag-search').value;
  $('tag-search').value = '';
  $('tag-error').hidden = true;
  showTagOptions();
  $('tag-search').focus();
  tagPicker.target?.create(name, (e) => {
    if (!$('tag-search').value) $('tag-search').value = name;
    showTagOptions();
    showTagError(e);
  });
}

$('viewer-tag-add').onclick = () => ($('tag-picker').hidden ? openTagPicker() : closeTagPicker());
$('tag-search').oninput = () => {
  $('tag-error').hidden = true;
  showTagOptions();
};
$('tag-create').onclick = createTag;
// In the picker: the arrows move between the search field, the tags and
// "Create", Enter in the search field creates the tag it offers, and Esc closes it.
$('tag-picker').addEventListener('keydown', (e) => {
  const options = [...$('tag-options').querySelectorAll('[data-tag]'), $('tag-create')].filter((option) => !option.hidden);
  const at = options.indexOf(document.activeElement);
  if (e.key === 'Escape') closeTagPicker();
  else if (e.key === 'Enter' && e.target === $('tag-search') && !$('tag-create').hidden) createTag();
  else if (e.key === 'ArrowDown' && e.target === $('tag-search')) options[0]?.focus();
  else if ((e.key === 'ArrowDown' || e.key === 'ArrowRight') && at >= 0) options[Math.min(at + 1, options.length - 1)].focus();
  else if ((e.key === 'ArrowUp' || e.key === 'ArrowLeft') && at > 0) options[at - 1].focus();
  else if ((e.key === 'ArrowUp' || e.key === 'ArrowLeft') && at === 0) $('tag-search').focus();
  else return;
  e.preventDefault();
});
document.addEventListener('pointerdown', (e) => {
  if (!$('tag-picker').hidden && !tagPicker.box.contains(e.target)) closeTagPicker(false);
});

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
  $('trim-replace').disabled = $('trim-save').disabled;
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
  video.addEventListener('playing', () => stretchStart(video));
  video.addEventListener('pause', () => {
    if (stretch?.video === video) stretchEnd('paused');
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
  stretchFrame(video);
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
  if (stretch) stretch.skip = true;
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
// mute, F full screen, T the tag picker, and I, O and Space for the trim. Keys
// in the open tag picker are its own.
document.addEventListener('keydown', (e) => {
  if ($('view-viewer').hidden || e.ctrlKey || e.altKey || e.metaKey) return;
  if (e.target.matches('input[type="text"], input[type="number"], input[type="search"], select')) return;
  if (e.target.closest?.('#tag-picker')) return;
  const key = e.key;
  // A clip that plays (deleted ones too), and one that can be trimmed.
  const playing = trim.video != null;
  const trimming = !$('trim').hidden;
  if (key === 'Escape') {
    if (!$('tag-picker').hidden) closeTagPicker();
    else if (document.body.classList.contains('player-full')) setFull(false);
    else if (!$('delete-confirm').hidden) cancelDelete();
    else closeViewer();
  } else if (playing && key.toLowerCase() === 'm') {
    toggleMute();
  } else if (playing && key.toLowerCase() === 'f') {
    toggleFull();
  } else if (key.toLowerCase() === 't' && !$('viewer-tag-add').hidden && !document.body.classList.contains('player-full')) {
    openTagPicker();
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

// "Save and delete original" (friends' request of 2026-11-07): the trimmed copy is
// saved as usual while the video stays; then the viewer lets go of the original,
// which goes to Recently deleted, and the copy takes its name, date, Keep mark,
// achievement and tags. Undo brings the original back next to the trimmed clip.
$('trim-replace').onclick = async () => {
  const item = library.current;
  const exact = trimMode() === 'exact';
  $('trim-save').disabled = true;
  $('trim-replace').disabled = true;
  $('viewer-status').textContent = exact ? t('Re-encoding the trimmed copy…') : t('Saving the trimmed copy…');
  let copy;
  try {
    copy = await invoke('library_trim', { path: item.path, start: trim.start, end: trim.end, exact });
  } catch (e) {
    $('viewer-status').textContent = t('Trim failed: {error}', { error: e });
    showTrim();
    return;
  }
  releaseMedia();
  try {
    const result = await invoke('library_replace_with_trim', { original: item.path, copy: copy.path });
    await loadLibrary();
    openViewer(result.item);
    $('viewer-status').textContent = t('Saved {file} ({length})', { file: fileName(result.item.path), length: fmtDuration(result.item.durationMs) });
    activity(t('Trimmed clip saved; the original went to Recently deleted: {file}', { file: fileName(result.item.path) }), 'clip');
    const deleted = result.deleted?.path;
    toast(t('Trimmed clip saved; the original went to Recently deleted.'), deleted ? t('Undo') : null, deleted ? () => restoreItems([deleted]) : null);
  } catch (e) {
    await loadLibrary();
    const original = library.items.find((i) => i.path === item.path);
    openViewer(original ?? copy);
    $('viewer-status').textContent = t('The trimmed copy was saved as {file}, but the original could not be deleted: {error}', { file: fileName(copy.path), error: e });
  }
};

$('trim-save').onclick = async () => {
  const item = library.current;
  const exact = trimMode() === 'exact';
  $('trim-save').disabled = true;
  $('trim-replace').disabled = true;
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

// New, trimmed, kept, tagged or removed items: the grid follows; an open viewer
// keeps its video and only refreshes the details and the position. An item read
// while a tag change was waiting or done may hold the tags of before: the change's
// own event brings it again.
listen('library-changed', async () => {
  const tagsBefore = tagWrites.done;
  await loadLibrary();
  if (!$('view-viewer').hidden && library.current) {
    const fresh = [...library.items, ...library.deleted].find((i) => i.path === library.current.path);
    if (fresh && !tagWrites.waiting && tagWrites.done === tagsBefore) {
      library.current = fresh;
      showDetails(fresh);
    }
    showPosition();
  } else {
    renderLibrary();
  }
});

