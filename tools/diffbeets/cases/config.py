"""Diffbeets cases: config."""

ORDER = 100

from ._corpus import ROOT

ART_CASES = []
EXTRA_CONFIGS = {}
EXPECTED = {}

# 2.7: `beet config` and `config -d` as confuse dumps them (comments from
# beets' default config, 4-space indent, inline short lists, empty nulls).
EXTRA_CONFIGS.update({
    "config-fetchart.yaml": "plugins: [fetchart]\nfetchart:\n    maxwidth: 500\n",
    "config-lyrics.yaml": "plugins: [lyrics]\n",
    "config-smartplaylist.yaml": "plugins: [smartplaylist]\n",
    "config-web.yaml": "plugins: [web]\n",
    "config-inline.yaml": "plugins: [inline]\nitem_fields:\n    foo: track + 1\n",
    "config-bucket.yaml": "plugins: [bucket]\nbucket:\n    bucket_year: ['1990s']\n",
    "config-replaygain.yaml": "plugins: [replaygain]\n",
    "config-export.yaml": "plugins: [export]\n",
    "config-importfeeds.yaml": "plugins: [importfeeds]\n",
    "config-playlist.yaml": "plugins: [playlist, parentwork]\n",
    "config-many.yaml": ("plugins: [the, zero, missing, convert, embedart, duplicates,"
                         " ftintitle, scrub, edit, albumtypes, types]\n"),
})

for _cmd in ("config", "config -d"):
    EXPECTED[f"-c config-replaygain.yaml {_cmd}"] = (
        "reference beets' replaygain plugin fails to initialize with its "
        "default (empty) command backend ('replaygain.command must be one "
        "of ...'), so its config dump ends with that error")


def _sort_fetchart_keys(text):
    """Sort fetchart's lazily added settings.

    beets adds `fanarttv_key`, `google_key`, `google_engine` and
    `lastfm_key` in the iteration order of a Python set of source classes,
    which depends on the process's memory layout; sort those lines inside
    the `fetchart:` block so the config cases are deterministic.
    """
    import re
    keys = re.compile(r"^ {4}(fanarttv_key|google_key|google_engine|lastfm_key):")
    lines = text.splitlines()
    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        out.append(line)
        i += 1
        if line == "fetchart:":
            block = []
            lazy = []
            while i < len(lines) and (not lines[i] or lines[i].startswith(" ")):
                block.append(lines[i])
                if keys.match(lines[i]):
                    lazy.append(lines[i])
                i += 1
            lazy.sort()
            it = iter(lazy)
            for l in block:
                if keys.match(l):
                    out.append(next(it))
                else:
                    out.append(l)
    return "\n".join(out) + ("\n" if text.endswith("\n") else "")


# Per-case normalizers for output the two tools order differently.
NORMALIZERS = {
    "-c config-fetchart.yaml config": _sort_fetchart_keys,
    "-c config-fetchart.yaml config -d": _sort_fetchart_keys,
}


def _config_cases():
    out = []
    for name in ["config-fetchart.yaml", "config-lyrics.yaml", "config-smartplaylist.yaml",
                 "config-web.yaml", "config-inline.yaml", "config-bucket.yaml",
                 "config-replaygain.yaml", "config-export.yaml", "config-playlist.yaml",
                 "config-importfeeds.yaml",
                 "config-many.yaml"]:
        c = ["-c", f"{ROOT}/{name}"]
        out.append((f"-c {name} config", [*c, "config"]))
        out.append((f"-c {name} config -d", [*c, "config", "-d"]))
    return out


# 2.8: deleting a flexible attribute from an item whose album has one of
# the same name: the album's value shows through (`foo: 2 -> 1`), or
# nothing changes when only the album has it.
ART_CASES += [
    ("modify-delete-album-attr", "tracks", {}, "{{plugins: []}}", [
        ["import", "-A", "-q", "{music}"],
        ["modify", "-y", "-a", "foo=1"],
        ["modify", "-y", "foo=2"],
        ["modify", "-y", "foo!"],
        ["ls", "-f", "$title|$foo"],
        ["modify", "-y", "bar!"],
        ["modify", "-y", "-a", "foo!"],
        ["ls", "-f", "$title|$foo"],
        ["modify", "-y", "-a", "genres=Rock"],
        ["modify", "-y", "genres!"],
    ]),
]
