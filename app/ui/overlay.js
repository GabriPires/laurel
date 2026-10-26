// In-game notices: the app emits "notice" { id, kind, title, detail, holdMs } and
// shows this window; each notice leaves after holdMs. At most three at a time.
const { listen } = window.__TAURI__.event;

const MAX_VISIBLE = 3;
const ICONS = {
  clip: '<path d="M5 12l4 4 10-10"/>',
  'auto-clip': '<path d="M8 4h8v5a4 4 0 01-8 0V4z"/><path d="M8 6H5.5a2.5 2.5 0 002.7 3.5M16 6h2.5a2.5 2.5 0 01-2.7 3.5"/><path d="M12 13v4M8.5 20h7M10 17h4"/>',
  screenshot: '<path d="M4 8h3l2-3h6l2 3h3v11H4z"/><circle cx="12" cy="13" r="3.5"/>',
  info: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3"/>',
  error: '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9L2.6 17a2 2 0 001.7 3h15.4a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z"/>',
};
const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

function dismiss(item) {
  if (!item.isConnected || item.classList.contains('leaving')) return;
  if (reducedMotion) return item.remove();
  item.classList.add('leaving');
  item.addEventListener('animationend', () => item.remove(), { once: true });
}

listen('notice', ({ payload }) => {
  const stack = document.getElementById('stack');
  const item = document.createElement('li');
  item.className = `toast ${payload.kind}`;
  item.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[payload.kind] ?? ICONS.info}</svg><div class="text"><span class="title"></span><span class="detail"></span></div>`;
  item.querySelector('.title').textContent = payload.title;
  item.querySelector('.detail').textContent = payload.detail;
  stack.prepend(item);
  const live = [...stack.children].filter((li) => !li.classList.contains('leaving'));
  live.slice(MAX_VISIBLE).forEach(dismiss);
  setTimeout(() => dismiss(item), payload.holdMs);
});
