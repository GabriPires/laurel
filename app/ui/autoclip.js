// Settings → Auto-clip (spec autoclip.steam-achievements): what Laurel watches now,
// and the installed Steam games, each with a switch to leave its achievements out.

function renderAutoclip(o) {
  $('steam-status').textContent = o.watching
    ? (o.achievements != null
      ? t('Watching the achievements of {game}: {unlocked} of {n} unlocked.', { game: o.watching, unlocked: o.unlocked, n: o.achievements })
      : t('Watching the achievements of {game}.', { game: o.watching }))
    : t('Laurel watches the achievements of the Steam game being recorded.');

  const list = $('steam-games');
  list.replaceChildren(...o.steamGames.map((g) => {
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.className = 'switch';
    box.setAttribute('role', 'switch');
    box.checked = !g.excluded;
    box.setAttribute('aria-label', t('Clip achievements in {game}', { game: g.name }));
    box.onchange = () => setExcluded(g.key, g.name, !box.checked);
    return gameItem(g.name, box);
  }));
  if (!o.steamGames.length) list.replaceChildren(gameItem(t('No Steam games found')));
}

async function setExcluded(key, name, excluded) {
  settings.steamExcluded = settings.steamExcluded.filter((g) => g.key !== key);
  if (excluded) settings.steamExcluded.push({ key, name });
  await saveSettings();
  loadAutoclip();
}

async function loadAutoclip() {
  renderAutoclip(await invoke('autoclip_status'));
}

const openSettingsBeforeAutoclip = window.openSettings;
window.openSettings = (...args) => {
  openSettingsBeforeAutoclip?.(...args);
  loadAutoclip();
};
for (const event of ['autoclip-state', 'game-state']) {
  listen(event, () => { if (!$('view-settings').hidden) loadAutoclip(); });
}
