#!/usr/bin/env python3
"""Checks Laurel's translations (app/i18n/<lang>.json) against the texts in the code.

The English text is the key (app/src/i18n.rs). The texts come from:
  - app/ui/index.html: text and the title, aria-label and placeholder attributes,
    except under script, style, code, [data-i18n-skip] and kbd (unless [data-i18n]),
    and the key an element names in data-i18n="key" (instead of its text);
  - app/ui/*.js: t('...') and tp(n, '...', '...');
  - app/src/*.rs: t!("..."), t_later!("...") and tr_plural(n, "...", "...").

It reports the texts with no translation, the translations nothing uses, and
translations whose {name} fields differ from the English ones. A text with no
lowercase word (H.264, LAUREL, 30 fps) or in SAME stays as it is. A key may end in
|context (see app/src/i18n.rs).

Usage: python3 -I tools/i18n-check.py [--lang pt-BR] [--missing-json]
  --missing-json prints the missing texts as a JSON skeleton to fill in.
Exit code 1 when something is missing, unused or mismatched.
"""

import argparse
import html.parser
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "app"

# Names and technical texts that read the same in every language.
SAME = {"Laurel", "FFmpeg", "Steam", "Epic", "Riot", "Xbox", "libobs", "Cinzel", "Figtree", "JetBrains Mono"}
UNITS = re.compile(r"\b(fps|kbps|GB|MB|ms|px)\b")
FIELD = re.compile(r"\{(\w+)\}")


def needs_translation(text):
    text = text.rsplit("|", 1)[0]
    if text in SAME:
        return False
    return re.search(r"[a-zà-ÿ]{2,}", UNITS.sub("", text)) is not None


class HtmlTexts(html.parser.HTMLParser):
    SKIP_TAGS = {"script", "style", "kbd", "code"}
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"}

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack = []  # (tag, skipped)
        self.texts = set()

    def skipped(self):
        return any(skip for _, skip in self.stack)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("data-i18n") and not self.skipped():
            self.texts.add(attrs["data-i18n"])
        skip = (tag in self.SKIP_TAGS and not (tag == "kbd" and "data-i18n" in attrs)) or "data-i18n-skip" in attrs or tag == "svg" or bool(attrs.get("data-i18n"))
        if not (skip or self.skipped()):
            for name in ("title", "aria-label", "placeholder"):
                if attrs.get(name):
                    self.texts.add(attrs[name])
        if tag not in self.VOID:
            self.stack.append((tag, skip))

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in self.VOID and self.stack and self.stack[-1][0] == tag:
            self.stack.pop()

    def handle_endtag(self, tag):
        for i in range(len(self.stack) - 1, -1, -1):
            if self.stack[i][0] == tag:
                del self.stack[i:]
                break

    def handle_data(self, data):
        text = " ".join(data.split())
        if text and not self.skipped():
            self.texts.add(text)


def unescape_js(s):
    return re.sub(r"\\(.)", lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)), s)


JS_STRING = r"""(['"])((?:\\.|(?!\1).)*)\1"""
JS_T = re.compile(r"\bt\(\s*" + JS_STRING)
JS_TP = re.compile(r"\btp\([^,()]*(?:\([^()]*\))?[^,()]*,\s*" + JS_STRING + r"\s*,\s*" + JS_STRING.replace("\\1", "\\3"))
RS_STRING = r'"((?:\\.|[^"\\])*)"'
RS_T = re.compile(r"\bt(?:_later)?!\(\s*" + RS_STRING)
RS_TP = re.compile(r"\btr_plural\([^,]*,\s*" + RS_STRING + r"\s*,\s*" + RS_STRING)


def collect():
    texts, plurals = {}, {}  # text -> sources; other -> (one, sources)

    def add(text, where):
        texts.setdefault(text, set()).add(where)

    parser = HtmlTexts()
    parser.feed((APP / "ui" / "index.html").read_text(encoding="utf-8"))
    for text in parser.texts:
        add(text, "index.html")
    for path in sorted((APP / "ui").glob("*.js")):
        if path.name == "i18n.js":
            continue
        code = path.read_text(encoding="utf-8")
        for m in JS_T.finditer(code):
            add(unescape_js(m.group(2)), path.name)
        for m in JS_TP.finditer(code):
            one, other = unescape_js(m.group(2)), unescape_js(m.group(4))
            plurals.setdefault(other, [one, set()])[1].add(path.name)
    for path in sorted((APP / "src").glob("*.rs")):
        code = path.read_text(encoding="utf-8")
        for m in RS_T.finditer(code):
            add(unescape_js(m.group(1)), path.name)
        for m in RS_TP.finditer(code):
            one, other = unescape_js(m.group(1)), unescape_js(m.group(2))
            plurals.setdefault(other, [one, set()])[1].add(path.name)
    return texts, plurals


def fields(s):
    return set(FIELD.findall(s))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lang", default="pt-BR")
    ap.add_argument("--missing-json", action="store_true")
    args = ap.parse_args()
    dictionary = json.loads((APP / "i18n" / f"{args.lang}.json").read_text(encoding="utf-8"))
    texts, plurals = collect()

    missing, mismatched = [], []
    for text, where in sorted(texts.items()):
        if text in plurals or not needs_translation(text):
            continue
        value = dictionary.get(text)
        if value is None:
            missing.append((text, sorted(where)))
        elif not isinstance(value, str) or fields(value) != fields(text):
            mismatched.append((text, value))
    for other, (one, where) in sorted(plurals.items()):
        value = dictionary.get(other)
        if value is None:
            missing.append((other, sorted(where)))
        elif not isinstance(value, dict) or "one" not in value or "other" not in value:
            mismatched.append((other, value))
        elif fields(value["other"]) != fields(other) or fields(value["one"]) - {"n"} != fields(one) - {"n"}:
            mismatched.append((other, value))
    used = set(texts) | set(plurals)
    unused = sorted(k for k in dictionary if k not in used)

    if args.missing_json:
        skeleton = {text: ({"one": plurals[text][0], "other": text} if text in plurals else text) for text, _ in missing}
        print(json.dumps(skeleton, ensure_ascii=False, indent=2))
        return 0
    for text, where in missing:
        print(f"missing ({', '.join(where)}): {text}")
    for text, value in mismatched:
        print(f"fields or form differ: {text!r} -> {value!r}")
    for text in unused:
        print(f"unused: {text}")
    print(f"{args.lang}: {len(dictionary)} translations; {len(missing)} missing, {len(mismatched)} mismatched, {len(unused)} unused")
    return 1 if missing or mismatched or unused else 0


if __name__ == "__main__":
    sys.exit(main())
