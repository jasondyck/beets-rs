"""Diffbeets cases: plugins sharing beets' `mpd` config section.

mpdupdate and mpdstats both add defaults to `mpd` in their `__init__`, and
beets adds a plugin's defaults only when it is loaded, in `plugins:` order.
`beet config` must show each combination as beets does: mpdupdate alone,
mpdstats alone, and both in either order (the shared keys keep the first
plugin's values and positions).
"""

ORDER = 86

from ._corpus import ROOT

ART_CASES = []
EXPECTED = {}

# 5.21e.
EXTRA_CONFIGS = {
    "config-mpdupdate.yaml": "plugins: [mpdupdate]\n",
    "config-mpdstats.yaml": "plugins: [mpdstats]\n",
    "config-mpd-both.yaml": "plugins: [mpdupdate, mpdstats]\n",
    "config-mpd-both-rev.yaml": "plugins: [mpdstats, mpdupdate]\n",
}

_CONFIGS = ["config-mpdupdate.yaml", "config-mpdstats.yaml",
            "config-mpd-both.yaml", "config-mpd-both-rev.yaml"]


def CASES():
    out = []
    for name in _CONFIGS:
        c = ["-c", f"{ROOT}/{name}"]
        out.append((f"-c {name} config", [*c, "config"]))
        out.append((f"-c {name} config -d", [*c, "config", "-d"]))
    return out
