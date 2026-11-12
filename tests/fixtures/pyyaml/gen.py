"""Writes cases.json: PyYAML's safe_dump_all/safe_load_all results that
src/pyyaml.rs is tested against. Run with the reference venv's Python:
    ~/.cache/beets-reference/venv/bin/python -I tests/fixtures/pyyaml/gen.py
"""
import datetime
import json
import os

import yaml


def enc(v):
    if v is None:
        return {"none": 1}
    if isinstance(v, bool):
        return {"bool": v}
    if isinstance(v, int):
        return {"int": str(v)}
    if isinstance(v, float):
        return {"float": repr(v)}
    if isinstance(v, str):
        return {"str": v}
    if isinstance(v, (datetime.date, datetime.datetime)):
        return {"ts": str(v)}
    if isinstance(v, bytes):
        return {"bytes": v.hex()}
    if isinstance(v, list):
        return {"list": [enc(x) for x in v]}
    if isinstance(v, dict):
        return {"map": [[enc(k), enc(x)] for k, x in v.items()]}
    raise TypeError(type(v))


STRINGS = [
    "", "plain", "yes", "No", "on", "null", "~", "123", "1.5", "0x1f", "017", "1_000",
    "2020-01-01", "2020-01-01 10:00:00", "-", "- x", "a: b", "a #b", "#c", "x:", ":x",
    "'quoted'", '"dq"', "trailing ", " leading", "multi\nline", "ends\n", "\ttab",
    "tab\tin", "café ½ Ünïcödé", "日本語", "emoji 🎵", "\x07bell", "\u00a0nbsp", "a\u2028b",
    "{brace}", "[bracket]", "a,b", "!bang", "&amp", "*star", "|pipe", ">gt", "%pct", "@at",
    "`tick", "?q", "? q", "=", "<<", "back\\slash", "100% pure $dollar", "a,comma {brace}",
    "What? Now: <yes> | *star*", ".dotfile", "...", "---", "x" * 100,
    "word " * 30, "long " + "y" * 90 + " end", "Supercalifragilisticexpialidocious " * 7,
    "spaces  inside", "sp \n nl", "\r\n", "\x85", "\ufeffbom", "a\x00b",
]
VALUES = [
    0, 1, -5, 2016, 10**20, 0.0, 1.0, -1.5, 3.14159, 1e16, 1e-5, 123456.789, 1730000000.123456,
    float("inf"), float("-inf"), True, False, None, [], ["Rock"], ["a", "b; c", "yes"],
]
dump_cases = []
for s in STRINGS:
    doc = {"title": s, "id": 1}
    dump_cases.append([[enc(doc)], yaml.safe_dump_all([doc], allow_unicode=True,
                                                      default_flow_style=False)])
for v in VALUES:
    doc = {"v": v, "id": 2}
    dump_cases.append([[enc(doc)], yaml.safe_dump_all([doc], allow_unicode=True,
                                                      default_flow_style=False)])
docs = [{"id": 1, "title": "One", "track": 1, "genres": ["Rock", "Pop"]},
        {"id": 2, "title": "Two", "artist": "B", "album": None},
        {"album": "Hdr"}]
dump_cases.append([[enc(d) for d in docs], yaml.safe_dump_all(docs, allow_unicode=True,
                                                               default_flow_style=False)])
dump_cases.append([[enc({s: 1 for s in ["b", "a", "yes", "1", "B"]})],
                   yaml.safe_dump_all([{s: 1 for s in ["b", "a", "yes", "1", "B"]}],
                                      allow_unicode=True, default_flow_style=False)])

LOADS = [
    "id: 1\ntitle: x\n", "a: yes\nb: No\nc: on\nd: OFF\n", "a: 0x1F\nb: 017\nc: 1_000\nd: 0b101\ne: 1:30\n",
    "a: 1.5\nb: .inf\nc: -.Inf\nd: 1e5\ne: 1.0e+5\nf: 1_0.5\n", "a: ~\nb: null\nc:\n",
    "a: 2020-01-01\nb: 2020-01-01 10:00:00\nc: 2020-1-2T03:04:05.5Z\nd: 2001-12-14 21:59:43.10 -5\n",
    "a: '123'\nb: \"yes\"\nc: !!str 12\nd: !!int '7'\n", "a: [1, two, 'three']\nb: {x: 1}\n",
    "- 1\n- 2\n", "just a string\n", "", "---\n", "a: 1\n---\nb: 2\n", "a: &x 5\nb: *x\n",
    "!MALFORMED", "wellformed: yes, but invalid", "a: [1, 2\n", "a: b: c\n", "a:\n  - x\n - y\n",
    "\"unterminated\n", "a: 1\na: 2\n", "1: x\ntrue: y\n~: z\n", "a: !!binary aGVsbG8=\n",
    "a: !foo bar\n", "a: *nope\n", "{a: 1}: x\n", "x: |\n  lit\n  block\n", "x: >\n  folded\n  text\n",
    "title: 'it''s'\n", "x: \"\\u00e9\\n\"\n", "a\tb: 1\n", "- [a, b]\n- {c: d}\n",
    "key: value # comment\n", "x: -\n", "x: 1.0.0\n", "x: 12:30:00\n", "x: +12\n", "x: -0\n",
]
load_cases = []
for s in LOADS:
    try:
        load_cases.append([s, {"docs": [enc(d) for d in yaml.safe_load_all(s)]}])
    except yaml.YAMLError as e:
        load_cases.append([s, {"error": str(e)}])

with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "cases.json"), "w") as f:
    json.dump({"dump": dump_cases, "load": load_cases}, f, ensure_ascii=False, indent=0)
