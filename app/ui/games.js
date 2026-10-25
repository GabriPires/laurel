// Settings → Games (specs game.detection, game.manual-add): the games found in the
// store libraries, each with a switch to stop recording it; the games the user
// added; adding a program that is open now or an .exe file.
const STORES = { steam: 'Steam', epic: 'Epic', riot: 'Riot', xbox: 'Xbox', list: t('Laurel list'), user: t('Added by you') };

let gamesOverview = null;

function gameItem(name, ...rest) {
  const li = document.createElement('li');
  const title = document.createElement('span');
  title.className = 'game-name';
  title.textContent = name;
  li.append(title, ...rest);
  return li;
}

function span(className, text) {
  const el = document.createElement('span');
  el.className = className;
  el.textContent = text;
  return el;
}

function liveBadge(key) {
  return gamesOverview?.game?.key === key ? [span('live', t('Recording now'))] : [];
}

function renderGames(o) {
  gamesOverview = o;
  const counts = Object.entries(o.counts).map(([store, n]) => t('{n} on {store}', { n, store: STORES[store] ?? store }));
  $('games-summary').textContent = o.mode === 'detect'
    ? (counts.length
      ? t('Laurel finds the games installed through Steam, Epic, Riot and Xbox ({counts}), plus the games you add.', { counts: counts.join(', ') })
      : t('Laurel finds the games installed through Steam, Epic, Riot and Xbox, plus the games you add.'))
    : t('Game detection is off: Laurel records a fixed target set for testing.');

  const installed = $('games-installed');
  installed.replaceChildren(...o.installed.map((g) => {
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.className = 'switch';
    box.setAttribute('role', 'switch');
    box.checked = !g.ignored;
    box.setAttribute('aria-label', t('Record {game}', { game: g.name }));
    box.title = box.checked ? t('Recorded') : t('Not recorded|game');
    box.onchange = () => setIgnored(g.key, g.name, !box.checked);
    return gameItem(g.name, ...liveBadge(g.key), span('game-meta', STORES[g.source] ?? g.source), box);
  }));
  if (!o.installed.length) installed.replaceChildren(gameItem(t('No games found in the store libraries')));

  $('games-custom').replaceChildren(...o.custom.map((g) => {
    const remove = document.createElement('button');
    remove.className = 'btn btn-secondary';
    remove.textContent = t('Remove');
    remove.onclick = async () => renderGames(await invoke('games_remove', { path: g.path }));
    return gameItem(g.name, ...liveBadge(`user:${g.path.toLowerCase()}`), span('game-path', g.path), remove);
  }));

  // Ignored games that are not in the installed list (for example from Laurel's list).
  const listed = new Set(o.installed.map((g) => g.key));
  const others = o.ignored.filter((g) => !listed.has(g.key));
  $('games-ignored-field').hidden = others.length === 0;
  $('games-ignored').replaceChildren(...others.map((g) => {
    const restore = document.createElement('button');
    restore.className = 'btn btn-secondary';
    restore.textContent = t('Record again');
    restore.onclick = () => setIgnored(g.key, g.name, false);
    return gameItem(g.name, restore);
  }));
}

async function setIgnored(key, name, ignored) {
  try {
    renderGames(await invoke('games_set_ignored', { key, name, ignored }));
  } catch (e) {
    $('settings-status').textContent = t('Could not change the game: {error}', { error: e });
  }
}

async function loadRunning() {
  const running = await invoke('games_running');
  const select = $('games-running');
  select.replaceChildren(...running.map((p) => {
    const option = document.createElement('option');
    option.value = p.path;
    option.textContent = `${p.title} — ${fileName(p.path)}`;
    option.dataset.title = p.title;
    return option;
  }));
  if (!running.length) {
    const option = document.createElement('option');
    option.textContent = t('No other programs are open');
    option.disabled = true;
    select.append(option);
  }
  $('games-add-running').disabled = !running.length;
}

async function addGame(path, name) {
  try {
    renderGames(await invoke('games_add', { path, name }));
    loadRunning();
  } catch (e) {
    $('settings-status').textContent = t('Could not add the game: {error}', { error: e });
  }
}

$('games-add-running').onclick = () => {
  const option = $('games-running').selectedOptions[0];
  if (option?.value) addGame(option.value, option.dataset.title);
};
$('games-browse').onclick = async () => {
  const path = await invoke('pick_exe');
  if (path) addGame(path, null);
};

async function loadGames() {
  renderGames(await invoke('games_overview'));
  loadRunning();
}

const openSettingsBefore = window.openSettings;
window.openSettings = (...args) => {
  openSettingsBefore?.(...args);
  loadGames();
};
listen('game-state', () => { if (!$('view-settings').hidden) loadGames(); });
