"""A scripted text editor for diffbeets' edit cases (beets' `editor` option:
`<python> -I editor.py`), so both tools see the same edits.

Each run is one edit: the file it's given is shown on stderr (as the tool
dumped it), then changed by the next entry of `x_editor` in ./config.yaml
(the case's config; the count is kept in ./editor-count). An entry is a
list of [old, new] replacements, a string to write instead of the content,
or null to leave the file as it is.
"""

import os
import re
import sys

import yaml

path = sys.argv[-1]
with open(path, encoding="utf-8") as f:
    text = f.read()
# Import times differ between the tools' runs.
shown = re.sub(r"(?m)^added: .*$", "added: <time>", text)
sys.stderr.write(f"--- editor ---\n{shown}--- end ---\n")

count_file = os.path.join(os.getcwd(), "editor-count")
n = 0
if os.path.exists(count_file):
    with open(count_file) as f:
        n = int(f.read() or 0)
with open(count_file, "w") as f:
    f.write(str(n + 1))

with open(os.path.join(os.getcwd(), "config.yaml"), encoding="utf-8") as f:
    rules = (yaml.safe_load(f) or {}).get("x_editor") or []
rule = rules[n] if n < len(rules) else None
if isinstance(rule, str):
    text = rule
elif isinstance(rule, list):
    for old, new in rule:
        text = text.replace(old, new)
if rule is not None:
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
