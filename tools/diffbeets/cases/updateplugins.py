"""Diffbeets cases: update and play-count plugins.

Their commands talk to servers, so what can run offline is the
configuration dumps (defaults and redacted keys) and the commands' early
argument/setting errors, which happen before any request.
"""

ORDER = 85

from ._corpus import ROOT

ART_CASES = []
EXPECTED = {}

# 5.18c part 2.
EXTRA_CONFIGS = {
    "config-mpdupdate.yaml": "plugins: [mpdupdate]\n",
    "config-plexupdate.yaml": "plugins: [plexupdate]\n",
    "config-kodiupdate.yaml": "plugins: [kodiupdate]\n",
    "config-subsonicupdate.yaml": "plugins: [subsonicupdate]\n",
    "config-mbcollection.yaml": ("plugins: [mbcollection]\n"
                                 "musicbrainz: {{user: user, pass: pass}}\n"),
    "config-listenbrainz.yaml": ("plugins: [listenbrainz]\n"
                                 "listenbrainz: {{token: token, username: user}}\n"),
    "config-lastimport.yaml": "plugins: [lastimport]\nlastfm: {{user: user}}\n",
    "config-lastimport-empty.yaml": "plugins: [lastimport]\n",
}


def CASES():
    out = []
    for name in ["config-mpdupdate.yaml", "config-plexupdate.yaml",
                 "config-kodiupdate.yaml", "config-subsonicupdate.yaml",
                 "config-mbcollection.yaml", "config-listenbrainz.yaml",
                 "config-lastimport.yaml"]:
        c = ["-c", f"{ROOT}/{name}"]
        out.append((f"-c {name} config", [*c, "config"]))
        out.append((f"-c {name} config -d", [*c, "config", "-d"]))
    # The argument check and the missing last.fm user both fail before any
    # request is made.
    out.append(("-c config-subsonicupdate.yaml subsonicupdate anarg",
                ["-c", f"{ROOT}/config-subsonicupdate.yaml", "subsonicupdate", "anarg"]))
    out.append(("-c config-lastimport-empty.yaml lastimport",
                ["-c", f"{ROOT}/config-lastimport-empty.yaml", "lastimport"]))
    return out
