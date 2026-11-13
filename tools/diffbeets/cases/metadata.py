"""Diffbeets cases: metadata source plugins (5.19)."""

ORDER = 88

from ._corpus import ROOT
from .art import _IMP

CASES = []
ART_CASES = []
EXTRA_CONFIGS = {}

# deezer loads without network; discogs and spotify use a token file the
# plugin setup finds in the app directory (BEETSDIR/BEETSRSDIR), so they
# load offline too. (beatport's config dump isn't comparable: beets loads
# the plugin, which logs its deprecation warning, before running `config`;
# beets-rs' `config` doesn't load plugins at all.) The plugin's item types
# make the flex attributes ints and dates, which modify/ls then show as
# such.
EXTRA_CONFIGS["config-deezer.yaml"] = "plugins: [deezer]\n"
EXTRA_CONFIGS["discogs_token.json"] = '{{"token": "t", "secret": "s"}}'
EXTRA_CONFIGS["spotify_token.json"] = '{{"access_token": "x"}}'
EXTRA_CONFIGS["config-discogs.yaml"] = "plugins: [discogs]\n"
EXTRA_CONFIGS["config-spotify.yaml"] = "plugins: [spotify]\n"

CASES += [
    (f"-c {name} {cmd}",
     ["-c", f"{ROOT}/{name}", *cmd.split()])
    for name in ["config-deezer.yaml", "config-discogs.yaml", "config-spotify.yaml"]
    for cmd in ["config", "config -d"]
]

ART_CASES += [
    ("deezer-types", "tracks", {}, "{{plugins: [deezer]}}", [
        _IMP,
        ["modify", "deezer_track_rank=1234", "deezer_track_id=42",
         "deezer_updated=2020-01-02"],
        ["ls", "-f", "$title|$deezer_track_rank|$deezer_track_id|$deezer_updated"],
        ["ls", "-f", "$title", "deezer_track_rank:1000.."],
        ["ls", "-f", "$title", "deezer_updated:2020-01-01.."],
    ]),
]
