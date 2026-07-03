#!/usr/bin/env python3
"""Sonic Oxide i18n tooling.

The UI's translation tables are English-keyed `key = value` .conf files in
etc/i18n/ (gettext message shape, so .po round-trips are mechanical).

Commands:
  extract              list every i18n key used by the GPUI sources
  missing <lang>       keys used in source but absent from etc/i18n/<lang>.conf
  to-po <lang>         convert etc/i18n/<lang>.conf to <lang>.po on stdout
  from-po <file.po>    convert a .po back to .conf on stdout
"""

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "experiments/gpui-spike/src"
I18N = REPO / "etc/i18n"

# i18n.tr("...") / .tr("...") call sites, plus label keys routed through
# helpers that translate internally (a11y_ctl, setting_toggle, pane titles).
TR_RE = re.compile(r'\.tr\(\s*"((?:[^"\\]|\\.)*)"')
HELPER_RE = re.compile(
    r'(?:a11y_ctl|setting_toggle)\(\s*"[^"]*",\s*"((?:[^"\\]|\\.)*)"', re.S
)


def extract_keys():
    keys = set()
    for path in SRC.rglob("*.rs"):
        text = path.read_text()
        keys.update(m.group(1) for m in TR_RE.finditer(text))
        keys.update(m.group(1) for m in HELPER_RE.finditer(text))
    return sorted(keys)


def parse_conf(path):
    table = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            k, v = line.split("=", 1)
            if k.strip() and v.strip():
                table[k.strip()] = v.strip()
    return table


def cmd_extract():
    print("\n".join(extract_keys()))


def cmd_missing(lang):
    table = parse_conf(I18N / f"{lang}.conf")
    missing = [k for k in extract_keys() if k not in table]
    print("\n".join(missing))
    return 1 if missing else 0


def po_escape(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


def cmd_to_po(lang):
    table = parse_conf(I18N / f"{lang}.conf")
    print(f'msgid ""\nmsgstr ""\n"Language: {lang}\\n"\n"Content-Type: text/plain; charset=UTF-8\\n"\n')
    for k in sorted(table):
        print(f'msgid "{po_escape(k)}"')
        print(f'msgstr "{po_escape(table[k])}"\n')


def cmd_from_po(path):
    text = Path(path).read_text()
    pairs = re.findall(r'msgid "((?:[^"\\]|\\.)+)"\s*\nmsgstr "((?:[^"\\]|\\.)*)"', text)
    unescape = lambda s: s.replace('\\"', '"').replace("\\\\", "\\")
    for k, v in pairs:
        if v:
            print(f"{unescape(k)} = {unescape(v)}")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["extract"]:
        cmd_extract()
    elif args[:1] == ["missing"] and len(args) == 2:
        sys.exit(cmd_missing(args[1]))
    elif args[:1] == ["to-po"] and len(args) == 2:
        cmd_to_po(args[1])
    elif args[:1] == ["from-po"] and len(args) == 2:
        cmd_from_po(args[1])
    else:
        print(__doc__)
        sys.exit(2)
