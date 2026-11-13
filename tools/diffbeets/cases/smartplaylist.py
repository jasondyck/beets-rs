"""Diffbeets cases: smartplaylist."""

ORDER = 70

from .art import _IMP
from .art_plugins import _PLUGIN_LOAD

ART_CASES = []
EXPECTED = {}

# ---------------------------------------------------------------------------
# smartplaylist (art phase: fresh libraries; the playlists under out/ are
# compared byte for byte in the state). The two tracks are imported as an
# album and again as singletons, then varied. beets logs its playlists in
# set order, so multi-playlist steps compare stderr sorted.

def _spl_cfg(extra=""):
    """The smartplaylist case config, with `extra` smartplaylist settings."""
    return _SPL_CFG.replace("<EXTRA>", extra)


_SPL_CFG = """{{plugins: [smartplaylist], import: {{duplicate_action: keep}},
 smartplaylist: {{<EXTRA>playlist_dir: '{root}/out', playlists: [
  {{name: all.m3u, query: ''}},
  {{name: 'by/$albumartist:$album.m3u', query: ''}},
  {{name: singles.m3u, query: 'singleton:true title-'}},
  {{name: multi.m3u, query: ['title:One', 'singleton:false track-', 'title:One']}},
  {{name: albums.m3u, album_query: 'album:Art', query: 'title:Tw'}},
  {{name: empty.m3u, query: 'title:nothing'}},
  {{name: noquery}},
  {{query: x}},
  {{name: bad.m3u, query: 'year:1..x'}},
  {{name: '$title.m3u', query: 'singleton:true'}}]}}}}"""
_SPL_SETUP = [
    _IMP,
    ["import", "-A", "-q", "-s", "{music}"],
    ["modify", "-y", "singleton:true", "title:Two", "title=Twö ?&#", "genres=Rock; Pop",
     "artist=Other/Act", "rating=5", "rg_track_gain=-6.5"],
]
ART_CASES += [
    ("spl-update", "tracks", {}, _spl_cfg(), [
        *_SPL_SETUP,
        (["splupdate"], "", True),
        (["splupdate", "-p"], "", True),
        ["splupdate", "tagada"],
        (["splupdate", "all", "singles.m3u"], "", True),
    ]),
    ("spl-options", "tracks", {}, _spl_cfg(), [
        *_SPL_SETUP,
        (["splupdate", "--output", "extm3u", "--prefix", "http://x/", "--urlencode",
          "--relative-to", "{root}/lib", "-d", "{root}/out/ext", "all", "singles"], "", True),
        ["splupdate", "--dest-regen", "--forward-slash", "-d", "{root}/out/regen", "all"],
        ["splupdate", "--uri-format", "http://h/item/$id/file", "-d", "{root}/out/uri", "all"],
        ["splupdate", "--relative-to", "{root}/out", "-d", "out/rel", "albums"],
        ["splupdate", "-f", "$title|$id", "multi"],
    ]),
    ("spl-fields", "tracks", {},
     _spl_cfg("output: extm3u, fields: [id, genres, length, comp, title, singleton,"
              " rg_track_gain, album_id, format, initial_key, bitrate], "), [
        *_SPL_SETUP,
        (["splupdate"], "", True),
    ]),
    ("spl-bad-output", "tracks", {}, _spl_cfg("output: xspf, "), [
        _IMP, ["splupdate"],
    ]),
]
EXPECTED.update({
    "art:spl-bad-output:1": _PLUGIN_LOAD,
    "art:spl-bad-output:2": _PLUGIN_LOAD,
    "art:spl-bad-output:state": _PLUGIN_LOAD,
})

# 5.18a: a plugin whose config is invalid fails to load, as fetchart's.
_PLUGIN_LOAD_518A = ("a plugin that fails to set up (an invalid rewrite rule or bucket "
                     "range): beets logs \"** error loading plugin\" with a Python traceback "
                     "and runs without it; beets-rs logs the same line with the error message")
EXPECTED.update({c: _PLUGIN_LOAD_518A for c in [
    "rewrite-bad.yaml load", "migrate:rewrite-bad.yaml load",
    "bucket-bad.yaml load", "migrate:bucket-bad.yaml load",
]})
