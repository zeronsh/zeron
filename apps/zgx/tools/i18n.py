#!/usr/bin/env python3
"""Zeron's words (ui/locales): keep the catalog whole while it grows.

  tools/i18n.py merge <fragment.json>   add a fragment's keys to en.json (a
                                        lock held: several editors at once);
                                        each new key goes to every other
                                        locale too, in English, until it's
                                        translated (so `zgx lint` stays clean)
  tools/i18n.py check [--strict]        every ui.t key the scripts name is in
                                        each locale; each locale's messages
                                        name the source's arguments; which
                                        keys are still English (--strict:
                                        exit 1 on any of them)
  tools/i18n.py pseudo [--keep]         ui/locales/en-XA.json: every message
                                        accented and 40% longer, in [ ], for
                                        layout checks (what isn't accented
                                        wasn't extracted). Ask for it with
                                        ui.locale:set("en-XA"). Not shipped:
                                        delete it after (or --keep).
  tools/i18n.py keys <prefix>           the keys under a prefix, with their
                                        English, to reuse rather than repeat
  tools/i18n.py apply <locale> <file>   a translation fragment (nested, the
                                        source's keys) into <locale>.json: only
                                        keys the source has, each naming the
                                        source's arguments, else refused

Messages are ICU MessageFormat (zgx's `ui.t`): `{name}`, `{n, plural, one {#
file} other {# files}}`, `{x, select, a {…} other {…}}`.
"""

import fcntl
import json
import os
import re
import sys
from collections import OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UI = os.path.join(ROOT, "ui")
# Where the scripts naming the catalog's keys are (a plugin's own folder,
# with --locales).
SCRIPTS = UI
LOCALES = os.path.join(UI, "locales")
SOURCE = os.path.join(LOCALES, "en.json")
PSEUDO = "en-XA"

# Kept in English in every locale: product and proper names, and what the
# source spells in another language already.
SAME_OK = re.compile(
    r"^(Zeron|Appshots|GitHub|Cloud|Claude|Codex|Claude Code|Gemini|Grok|Cursor|OpenCode|Amp|"
    r"Git|Slack|Linear|macOS|Mac|iOS|Android|Linux|Windows|Geist|English|简体中文|"
    r"OK|URL|API|PR|SSH|MCP|JSON|PNG|PDF)$"
)


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f, object_pairs_hook=OrderedDict)


def dump(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")
    os.replace(tmp, path)


def flat(d, prefix=""):
    out = OrderedDict()
    for k, v in d.items():
        key = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            out.update(flat(v, key))
        else:
            out[key] = v
    return out


def merge_into(base, frag, fill_only=False, path=""):
    """Deep-merges frag into base. fill_only: only keys base lacks. Returns
    the keys whose English changed (a translation to look at again)."""
    changed = []
    for k, v in frag.items():
        key = f"{path}.{k}" if path else k
        if isinstance(v, dict):
            if k in base and not isinstance(base[k], dict):
                raise SystemExit(f"{key}: a message in the catalog, a group in the fragment")
            base.setdefault(k, OrderedDict())
            changed += merge_into(base[k], v, fill_only, key)
        else:
            if k in base and isinstance(base[k], dict):
                raise SystemExit(f"{key}: a group in the catalog, a message in the fragment")
            if k in base and base[k] != v and not fill_only:
                changed.append(key)
            if not fill_only or k not in base:
                base[k] = v
    return changed


def others():
    return [
        os.path.join(LOCALES, n)
        for n in sorted(os.listdir(LOCALES))
        if n.endswith(".json") and n != "en.json" and not n.startswith(PSEUDO)
    ]


# The design system's English, for a VM without the catalog (ds/locale.luau:
# a plugin's, loaded before the app's): the keys under these.
FALLBACK_PREFIXES = ("ds.", "common.", "time.", "language.")
FALLBACK = os.path.join(UI, "ds", "locale_en.luau")


def luau_string(v):
    return '"' + v.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r") + '"'


def write_fallback(en=None):
    en = flat(en if en is not None else load(SOURCE))
    lines = [
        "-- Made by tools/i18n.py from ui/locales/en.json: don't edit. The design",
        "-- system's English, for a VM with no catalog yet (ds/locale.luau).",
        "-- zgx-lint: allow(*)",
        "return {",
    ]
    for k, v in en.items():
        if k.startswith(FALLBACK_PREFIXES):
            lines.append(f"\t[{luau_string(k)}] = {luau_string(v)},")
    lines.append("} :: { [string]: string }")
    text = "\n".join(lines) + "\n"
    old = open(FALLBACK, encoding="utf-8").read() if os.path.exists(FALLBACK) else None
    if old != text:
        tmp = FALLBACK + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, FALLBACK)


def cmd_merge(paths):
    lock = open(os.path.join(LOCALES, ".lock"), "w")
    fcntl.flock(lock, fcntl.LOCK_EX)
    try:
        en = load(SOURCE)
        for p in paths:
            frag = load(p)
            for k, v in flat(frag).items():
                problem = parse_problem(v)
                if problem:
                    raise SystemExit(f"{p}: {k}: {problem}")
            changed = merge_into(en, frag)
            for key in changed:
                print(f"changed English: {key} (its translations are now out of date)")
            for other in others():
                data = load(other)
                merge_into(data, frag, fill_only=True)
                dump(other, data)
        dump(SOURCE, en)
        write_fallback(en)
        print(f"en.json: {len(flat(en))} keys")
    finally:
        fcntl.flock(lock, fcntl.LOCK_UN)


# --- ICU, enough to compare arguments and keep syntax when transforming ---

def parse(msg):
    """Parses msg into a list of parts: str, or (name, type, branches) with
    branches an OrderedDict of key -> parts (plural/select), or None."""
    pos = 0

    def text(depth, in_plural):
        nonlocal pos
        parts, buf = [], []
        while pos < len(msg):
            c = msg[pos]
            if c == "{":
                if buf:
                    parts.append("".join(buf))
                    buf = []
                parts.append(argument())
            elif c == "}":
                if depth == 0:
                    raise ValueError(f"unmatched }} at {pos}")
                break
            elif c == "'" and pos + 1 < len(msg) and msg[pos + 1] == "'":
                buf.append("''")
                pos += 2
            elif c == "'" and pos + 1 < len(msg) and msg[pos + 1] in "{}#|":
                # Quoted up to the next lone apostrophe.
                end = msg.find("'", pos + 1)
                end = len(msg) if end < 0 else end + 1
                buf.append(msg[pos:end])
                pos = end
            elif c == "#" and in_plural:
                if buf:
                    parts.append("".join(buf))
                    buf = []
                parts.append(("#", "hash", None))
                pos += 1
            else:
                buf.append(c)
                pos += 1
        if buf:
            parts.append("".join(buf))
        return parts

    def argument():
        nonlocal pos
        pos += 1  # {
        m = re.compile(r"\s*([\w.]+)\s*").match(msg, pos)
        if not m:
            raise ValueError(f"no argument name at {pos}")
        name = m.group(1)
        pos = m.end()
        if pos < len(msg) and msg[pos] == "}":
            pos += 1
            return (name, None, None)
        if msg[pos] != ",":
            raise ValueError(f"expected , or }} at {pos}")
        pos += 1
        m = re.compile(r"\s*(\w+)\s*").match(msg, pos)
        kind = m.group(1)
        pos = m.end()
        if kind in ("plural", "select", "selectordinal"):
            if msg[pos] != ",":
                raise ValueError(f"expected , after {kind}")
            pos += 1
            branches = OrderedDict()
            while True:
                m = re.compile(r"\s*([=\w]+)\s*").match(msg, pos)
                if not m:
                    break
                key = m.group(1)
                pos = m.end()
                if msg[pos] != "{":
                    raise ValueError(f"expected {{ after {key}")
                pos += 1
                branches[key] = text(1, kind != "select")
                if pos >= len(msg) or msg[pos] != "}":
                    raise ValueError("unclosed branch")
                pos += 1
            m = re.compile(r"\s*\}").match(msg, pos)
            if not m:
                raise ValueError(f"unclosed {kind}")
            pos = m.end()
            return (name, kind, branches)
        # number, date: up to the closing brace.
        end = msg.find("}", pos)
        style = msg[pos:end]
        pos = end + 1
        return (name, kind + style, None)

    parts = text(0, False)
    if pos < len(msg):
        raise ValueError(f"unmatched }} at {pos}")
    return parts


def parse_problem(msg):
    try:
        parse(msg)
        return None
    except (ValueError, IndexError, AttributeError) as e:
        return f"doesn't parse: {e}"


def arguments(parts, out=None):
    out = set() if out is None else out
    for p in parts:
        if isinstance(p, tuple) and p[1] != "hash":
            out.add(p[0])
            if p[2]:
                for b in p[2].values():
                    arguments(b, out)
    return out


def scripts():
    for dirpath, dirnames, filenames in os.walk(SCRIPTS):
        dirnames[:] = [d for d in dirnames if d not in (".zgx", "locales")]
        for n in filenames:
            if n.endswith(".luau"):
                yield os.path.join(dirpath, n)


# ui.t in screens, the design system's locale.t / locale.s (ds/locale.luau).
T_CALL = re.compile(r"""(?:ui\.t|locale\.[ts]|zeron\.[ts])\(\s*(["'`])([^"'`]+)\1""")


def used_keys():
    keys = {}
    for path in scripts():
        with open(path, encoding="utf-8") as f:
            for i, line in enumerate(f, 1):
                # (Not what a comment shows as an example.)
                line = re.sub(r"--.*$", "", line) if "--" in line and re.search(r"(ui\.t|locale\.[ts]|zeron\.[ts])\(", line.split("--", 1)[1]) else line
                for m in T_CALL.finditer(line):
                    if SCRIPTS != UI and not m.group(0).startswith("zeron."):
                        continue  # (a plugin's own keys are zeron.t's)
                    keys.setdefault(m.group(2), f"{os.path.relpath(path, ROOT)}:{i}")
    return keys


def looks_english(v):
    """A translation that still reads as the source's English: Latin
    letters outside placeholders, product names and keys."""
    try:
        parts = parse(v)
    except Exception:
        return True
    words = []

    def walk(ps):
        for p in ps:
            if isinstance(p, str):
                words.extend(re.findall(r"[A-Za-z][A-Za-z']+", p))
            elif p[2]:
                for b in p[2].values():
                    walk(b)

    walk(parts)
    return [w for w in words if not SAME_OK.match(w)]


def cmd_check(strict):
    en = flat(load(SOURCE))
    bad = 0
    for k, v in en.items():
        problem = parse_problem(v)
        if problem:
            print(f"en: {k}: {problem}")
            bad += 1
    used = used_keys()
    for k, where in sorted(used.items()):
        if "{" in k:
            continue  # (a key built from code: listed nowhere to check)
        if k not in en:
            print(f"missing in en: {k} ({where})")
            bad += 1
    english_left = 0
    for other in others():
        name = os.path.basename(other)
        data = flat(load(other))
        for k in en:
            if k not in data:
                print(f"{name}: missing {k}")
                bad += 1
        for k in data:
            if k not in en:
                print(f"{name}: extra {k}")
                bad += 1
        for k, v in data.items():
            if k not in en:
                continue
            problem = parse_problem(v)
            if problem:
                print(f"{name}: {k}: {problem}")
                bad += 1
                continue
            a, b = arguments(parse(en[k])), arguments(parse(v))
            if a != b:
                print(f"{name}: {k}: arguments {sorted(b)}, English has {sorted(a)}")
                bad += 1
            if v == en[k] and looks_english(v) or (v != en[k] and looks_english(v) and not k.endswith(".terms")):
                english_left += 1
                if strict or os.environ.get("I18N_VERBOSE"):
                    print(f"{name}: still English: {k} = {v!r}")
    unused = [k for k in en if k not in used]
    print(
        f"{len(en)} keys, {len(used)} named in scripts, {len(unused)} not named by a literal ui.t "
        f"(built keys or unused), {english_left} still English in the other locales, {bad} problems"
    )
    if bad or (strict and english_left):
        sys.exit(1)


ACCENTS = dict(zip("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
                   "áƀçđéƒĝĥíĵķĺɱñóþǫŕšţúṽŵẋýžÁƁÇĐÉƑĜĤÍĴĶĹṀÑÓÞǪŔŠŢÚṼŴẊÝŽ"))


def pseudo_text(s):
    out = "".join(ACCENTS.get(c, c) for c in s)
    return out


def pseudo_parts(parts):
    out = []
    for p in parts:
        if isinstance(p, str):
            out.append(pseudo_text(p))
        elif p[1] == "hash":
            out.append("#")
        elif p[2] is None:
            out.append("{" + p[0] + "}" if p[1] is None else "{" + p[0] + ", " + p[1] + "}")
        else:
            branches = " ".join(f"{k} {{{pseudo_parts(v)}}}" for k, v in p[2].items())
            out.append("{" + p[0] + ", " + p[1] + ", " + branches + "}")
    return "".join(out)


def pseudo(v):
    body = pseudo_parts(parse(v))
    plain = re.sub(r"\{[^{}]*\}", "", v)
    pad = "·" * max(1, round(len(plain) * 0.4))
    return f"[{body} {pad}]"


def unflat(d):
    out = OrderedDict()
    for k, v in d.items():
        node = out
        parts = k.split(".")
        for p in parts[:-1]:
            node = node.setdefault(p, OrderedDict())
        node[parts[-1]] = v
    return out


def cmd_pseudo():
    en = flat(load(SOURCE))
    out = OrderedDict((k, pseudo(v)) for k, v in en.items())
    path = os.path.join(LOCALES, PSEUDO + ".json")
    dump(path, unflat(out))
    print(f"wrote {os.path.relpath(path, ROOT)} ({len(out)} messages); delete it when done")


def cmd_apply(locale, path):
    target = os.path.join(LOCALES, locale + ".json")
    lock = open(os.path.join(LOCALES, ".lock"), "w")
    fcntl.flock(lock, fcntl.LOCK_EX)
    try:
        en = flat(load(SOURCE))
        frag = flat(load(path))
        bad = []
        for k, v in frag.items():
            if k not in en:
                bad.append(f"{k}: not in en.json")
                continue
            if not isinstance(v, str):
                bad.append(f"{k}: not a string")
                continue
            problem = parse_problem(v)
            if problem:
                bad.append(f"{k}: {problem}")
                continue
            a, b = arguments(parse(en[k])), arguments(parse(v))
            if a != b:
                bad.append(f"{k}: arguments {sorted(b)}, English has {sorted(a)}")
        if bad:
            print("\n".join(bad))
            raise SystemExit(f"{len(bad)} refused; nothing applied")
        data = load(target) if os.path.exists(target) else OrderedDict()
        flat_data = flat(data)
        flat_data.update(frag)
        # In the source's order, so the files read side by side.
        ordered = OrderedDict((k, flat_data[k]) for k in en if k in flat_data)
        dump(target, unflat(ordered))
        print(f"{locale}.json: {len(frag)} applied")
    finally:
        fcntl.flock(lock, fcntl.LOCK_UN)


def cmd_keys(prefix):
    en = flat(load(SOURCE))
    for k, v in en.items():
        if k.startswith(prefix):
            print(f"{k} = {v}")


if __name__ == "__main__":
    args = sys.argv[1:]
    # Another catalog than Zeron's: a plugin's (`--locales
    # plugins-builtin/github/locales`), for check, apply and keys.
    if "--locales" in args:
        at = args.index("--locales")
        LOCALES = os.path.abspath(args[at + 1])
        SOURCE = os.path.join(LOCALES, "en.json")
        SCRIPTS = os.path.dirname(LOCALES)
        del args[at : at + 2]
    if not args:
        print(__doc__)
        sys.exit(2)
    cmd, rest = args[0], args[1:]
    if cmd == "merge":
        cmd_merge(rest)
    elif cmd == "check":
        cmd_check("--strict" in rest)
    elif cmd == "pseudo":
        cmd_pseudo()
    elif cmd == "apply":
        cmd_apply(rest[0], rest[1])
    elif cmd == "fallback":
        write_fallback()
    elif cmd == "keys":
        cmd_keys(rest[0] if rest else "")
    else:
        print(__doc__)
        sys.exit(2)
