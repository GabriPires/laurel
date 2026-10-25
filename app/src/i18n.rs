//! Interface language (Settings → Language): English, the language the code is
//! written in, or Brazilian Portuguese from i18n/pt-BR.json. The English text is the
//! key, so a text with no translation shows in English. A translation is a string,
//! or `{ "one": …, "other": … }` for a text with a count, keyed by its plural form.
//! `{name}` marks a value filled in at run time, in both languages. A key may end in
//! `|context` when one English text needs two translations ("Recording|status"
//! reads "Gravando", the page "Recording" reads "Gravação"); English drops it.
//!
//! The main window gets the same strings from `script()`, which the app serves before
//! the window's own scripts (main.rs, the "laurel" protocol). The overlay shows the
//! notices this side translates. tools/i18n-check.py lists the texts that have no
//! translation, the translations nothing uses, and mismatched `{name}` fields.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use serde_json::{json, Value};

/// The values of the language setting; "system" follows Windows.
pub const LANGUAGES: &[&str] = &["system", "en", "pt-BR"];

static PORTUGUESE: AtomicBool = AtomicBool::new(false);

fn portuguese() -> &'static HashMap<String, Value> {
    static STRINGS: OnceLock<HashMap<String, Value>> = OnceLock::new();
    STRINGS.get_or_init(|| serde_json::from_str(include_str!("../i18n/pt-BR.json")).unwrap_or_default())
}

/// The language a setting stands for.
pub fn resolve(setting: &str) -> &'static str {
    match setting {
        "en" => "en",
        "pt-BR" => "pt-BR",
        _ => system(),
    }
}

/// Windows' display language, when Laurel has it, else English.
pub fn system() -> &'static str {
    if crate::win::ui_language_is_portuguese() { "pt-BR" } else { "en" }
}

/// Switches the language of everything translated from now on.
pub fn set(setting: &str) {
    PORTUGUESE.store(resolve(setting) == "pt-BR", Ordering::SeqCst);
}

pub fn current() -> &'static str {
    if PORTUGUESE.load(Ordering::SeqCst) { "pt-BR" } else { "en" }
}

fn lookup(key: &str, count: Option<u64>) -> Option<&'static str> {
    if !PORTUGUESE.load(Ordering::SeqCst) {
        return None;
    }
    match portuguese().get(key)? {
        Value::String(s) => Some(s),
        forms => forms[if count == Some(1) { "one" } else { "other" }].as_str().or_else(|| forms["other"].as_str()),
    }
}

fn fill(text: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = text.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// The English text of a key: without its `|context`.
fn english(key: &str) -> &str {
    key.rsplit_once('|').map_or(key, |(text, _)| text)
}

pub fn tr(text: &str) -> String {
    lookup(text, None).unwrap_or(english(text)).to_string()
}

/// `text` translated, with its `{name}` fields filled in (see the `t!` macro).
pub fn tr_args(text: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(lookup(text, None).unwrap_or(english(text)), args)
}

/// The text for `n` things: `one` or `other` in English, keyed by `other`; `{n}` is
/// filled in with the count.
pub fn tr_plural(n: u64, one: &str, other: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let text = lookup(other, Some(n)).unwrap_or(english(if n == 1 { one } else { other }));
    fill(&fill(text, args), &[("n", &n)])
}

/// A number with `digits` decimals and the language's decimal mark.
pub fn number(x: f64, digits: usize) -> String {
    let s = format!("{x:.digits$}");
    if PORTUGUESE.load(Ordering::SeqCst) { s.replace('.', ",") } else { s }
}

/// The main window's first script: the language and its strings.
pub fn script() -> String {
    let strings = if PORTUGUESE.load(Ordering::SeqCst) { json!(portuguese()) } else { json!({}) };
    format!("window.LAUREL_I18N = {};\n", json!({ "lang": current(), "strings": strings }))
}

/// `t!("Last {secs} s", secs = n)`: translated with its fields filled in.
macro_rules! t {
    ($text:literal) => {
        $crate::i18n::tr($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::tr_args($text, &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+])
    };
}
pub(crate) use t;

/// A text translated where it is shown, with `tr` (it stays English here, for
/// example for the logs); marked so tools/i18n-check.py finds it.
macro_rules! t_later {
    ($text:literal) => {
        $text
    };
}
pub(crate) use t_later;
