// Interface language (Settings → Language). The app serves the language and its
// strings (app/i18n/<lang>.json, see app/src/i18n.rs) as the script before this one;
// without it everything stays English. English text is the key: t('Clip saved'),
// t('Last {secs} s', { secs }), tp(n, '{n} item', '{n} items'); a key may end in
// |context when one English text needs two translations (English drops it). The
// page's own text and its title, aria-label and placeholder attributes are
// translated here, before the other scripts run; elements under [data-i18n-skip],
// code and kbd keep theirs (a key name with [data-i18n] is translated), and an
// element with data-i18n="key" takes that key's text (a key with a |context).
const I18N = window.LAUREL_I18N ?? { lang: 'en', strings: {} };
const LANG = I18N.lang;
const LOCALE = LANG === 'en' ? 'en-US' : LANG;
const DECIMAL = (1.5).toLocaleString(LOCALE).charAt(1);

function i18nFill(text, params) {
  return params ? text.replace(/\{(\w+)\}/g, (field, name) => (name in params ? String(params[name]) : field)) : text;
}

// The English text of a key: without its |context.
const english = (key) => (key.includes('|') ? key.slice(0, key.lastIndexOf('|')) : key);

function t(text, params) {
  const s = I18N.strings[text];
  return i18nFill(typeof s === 'string' ? s : (s?.other ?? english(text)), params);
}

// A text for n things, keyed by its English plural; {n} is the count.
function tp(n, one, other, params) {
  const s = I18N.strings[other];
  let text = english(n === 1 ? one : other);
  if (typeof s === 'string') text = s;
  else if (s) text = (n === 1 ? s.one : s.other) ?? s.other;
  return i18nFill(text, { n: fmtNum(n), ...params });
}

// A number in the language's format, with `digits` decimals.
const fmtNum = (x, digits = 0) => x.toLocaleString(LOCALE, { minimumFractionDigits: digits, maximumFractionDigits: digits });

function translateDom(root) {
  if (!Object.keys(I18N.strings).length) return;
  for (const el of root.querySelectorAll('[data-i18n]:not([data-i18n=""])')) el.textContent = t(el.dataset.i18n);
  const skip = 'script, style, kbd:not([data-i18n]), code, [data-i18n-skip], [data-i18n]:not([data-i18n=""])';
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.parentElement?.closest(skip)) continue;
    const s = I18N.strings[node.nodeValue.replace(/\s+/g, ' ').trim()];
    if (typeof s !== 'string') continue;
    const [lead, trail] = [node.nodeValue.match(/^\s*/)[0], node.nodeValue.match(/\s*$/)[0]];
    node.nodeValue = lead + s + trail;
  }
  for (const el of root.querySelectorAll('[title], [aria-label], [placeholder]')) {
    if (el.closest('[data-i18n-skip]')) continue;
    for (const attr of ['title', 'aria-label', 'placeholder']) {
      const s = I18N.strings[el.getAttribute(attr)];
      if (typeof s === 'string') el.setAttribute(attr, s);
    }
  }
}

document.documentElement.lang = LANG;
translateDom(document.body);
