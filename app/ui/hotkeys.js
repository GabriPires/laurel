// Settings → Shortcuts (spec clip.hotkeys): shows the hotkeys and records a new one.
// While a hotkey is being recorded the backend suspends the hotkeys; the next valid
// combination is saved there, an invalid one comes back with the reason.
const HOTKEY_HELP = t('Shortcuts work in games, also in exclusive fullscreen, and the game still gets the key.');
let hotkeys = null;

function showHotkeys(h) {
  hotkeys = h;
  if (!h.available) {
    $('hotkey-message').textContent = t('Shortcuts are unavailable; see the log.');
    return;
  }
  for (const action of ['clip', 'screenshot', 'toggle']) {
    const capturing = h.capturing === action;
    const kbd = $(`hk-${action}`);
    kbd.textContent = capturing ? t('Press keys…') : (h[action]?.label ?? t('Not set'));
    kbd.classList.toggle('capturing', capturing);
    kbd.closest('.hotkey-row').classList.toggle('capturing', capturing);
    if ($(`kbd-${action}`)) $(`kbd-${action}`).textContent = h[action]?.label ?? '';
    const button = document.querySelector(`[data-hotkey="${action}"]`);
    button.textContent = capturing ? t('Cancel') : t('Change');
    button.disabled = h.capturing != null && !capturing;
  }
  $('hk-toggle-clear').hidden = !h.toggle || h.capturing != null;
}

const ACTION_LABELS = { clip: t('Save clip'), screenshot: t('Screenshot'), toggle: t('Recording on/off') };

function hotkeyMessage(text, bad = false) {
  $('hotkey-message').textContent = text;
  $('hotkey-message').closest('.note').classList.toggle('bad', bad);
}

for (const button of document.querySelectorAll('[data-hotkey]')) {
  button.onclick = async () => {
    const action = button.dataset.hotkey;
    if (hotkeys?.capturing === action) {
      await invoke('hotkey_cancel');
      return;
    }
    try {
      showHotkeys(await invoke('hotkey_capture', { action }));
      hotkeyMessage(t('Press the new shortcut: an F key, a key with Ctrl or Alt, or the middle or side mouse buttons. Esc cancels.'));
    } catch (e) {
      hotkeyMessage(`${e}`, true);
    }
  };
}

$('hk-toggle-clear').onclick = async () => {
  showHotkeys(await invoke('hotkey_clear'));
  hotkeyMessage(t('Recording on/off has no shortcut now'));
};

$('hotkeys-reset').onclick = async () => {
  showHotkeys(await invoke('hotkeys_reset'));
  hotkeyMessage(t('Shortcuts reset to F8 and Shift+F8'));
};

listen('hotkeys-changed', (e) => {
  const was = hotkeys?.capturing;
  showHotkeys(e.payload);
  if (was && !e.payload.capturing && e.payload[was]) hotkeyMessage(`${ACTION_LABELS[was]}: ${e.payload[was].label}`);
});
listen('hotkey-rejected', (e) => hotkeyMessage(t("{key} can't be used: {reason}. Try another, or press Esc.", { key: e.payload.label, reason: e.payload.reason }), true));
listen('hotkey-cancelled', async () => {
  showHotkeys(await invoke('hotkeys_get'));
  hotkeyMessage(HOTKEY_HELP);
});

invoke('hotkeys_get').then(showHotkeys);
