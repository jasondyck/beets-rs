"""Diffbeets cases: art plugins."""

ORDER = 50

from .imports import EDITOR
from ._corpus import ROOT
from .art import _IMP
import json

ART_CASES = []
EXTRA_CONFIGS = {}
EXPECTED = {}

# zero (state: the files' tags, as `write -p` shows them against the library).
_ZERO_LS = ["ls", "-f", "$title|$artist|$album|$year-$month|$disc/$disctotal|$comments|$genres"
            "|$catalognum"]
_ZERO_MOD = ["modify", "-y", "comments=hello", "genres=Rock", "catalognum=C1", "month=5"]
_ZERO_BACK = [["write", "-p"], _ZERO_LS]


def _zero(cfg):
    return "{{plugins: [zero], import: {{write: yes}}, zero: " + cfg + "}}"


ART_CASES += [
    ("zero-write", "tracks", {}, _zero("{{fields: [year, comments]}}"),
     [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-write-patterns", "tracks", {},
     _zero("{{fields: title comments genres, title: ['^o'], comments: [hel+o], "
           "genres: [\"'rock'\"]}}"), [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-write-db", "tracks", {}, _zero("{{fields: [month, comments], update_database: yes}}"),
     [_IMP, _ZERO_MOD, _ZERO_LS, *_ZERO_BACK]),
    ("zero-auto-off", "tracks", {}, _zero("{{fields: [comments], auto: no}}"),
     [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-keep", "tracks", {}, _zero("{{keep_fields: [title, artist, comments]}}"),
     [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-aliases", "tracks", {}, _zero("{{fields: [date, genre, catalognums, languages]}}"),
     [_IMP, _ZERO_MOD + ["language=eng"], *_ZERO_BACK, ["ls", "-f", "$language"]]),
    ("zero-single-disc", "tracks", {}, _zero("{{omit_single_disc: yes, fields: [comments]}}"),
     [_IMP, ["modify", "-y", "disc=1", "disctotal=1", "title:One"],
      ["modify", "-y", "disc=1", "disctotal=2", "title:Two"], *_ZERO_BACK]),
    ("zero-images", "tracks-art", {}, _zero("{{fields: [images]}}"), [_IMP, _ZERO_MOD]),
    ("zero-art", "tracks-art", {}, _zero("{{fields: [art]}}"), [_IMP, _ZERO_MOD]),
    ("zero-keep-images", "tracks-art", {}, _zero("{{keep_fields: [images]}}"),
     [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-config-warnings", "tracks", {},
     _zero("{{fields: [bogus, id, path, comments], keep_fields: [title]}}"),
     [_IMP, _ZERO_MOD, ["zero", "title:One"], _ZERO_LS]),
    ("zero-invalid-fields", "tracks", {}, _zero("{{fields: [bogus, id, comments]}}"),
     [_IMP, _ZERO_MOD, *_ZERO_BACK]),
    ("zero-no-fields", "tracks", {}, _zero("{{fields: []}}"),
     [_IMP, ["zero", "title:One"], (["zero"], "y\n")]),
    # The command.
    ("zero-cmd", "tracks", {}, _zero("{{fields: [comments, year], auto: no}}"),
     [_IMP, _ZERO_MOD, (["zero"], "y\n"), _ZERO_LS, *_ZERO_BACK]),
    ("zero-cmd-db", "tracks", {},
     _zero("{{fields: [comments, year], auto: no, update_database: yes}}"),
     [_IMP, _ZERO_MOD, ["zero", "title:One"], _ZERO_LS, *_ZERO_BACK]),
    ("zero-cmd-prompt", "tracks", {}, _zero("{{fields: [comments], update_database: yes}}"),
     [_IMP, (["zero"], "n\n"), (["zero"], "x\n\ny\n"), (["zero", "-l", "1"], "y\n"),
      _ZERO_LS]),
    ("zero-cmd-limit", "tracks", {}, _zero("{{fields: [title], update_database: yes}}"),
     [_IMP, ["zero", "--limit=1", "album:Art"], _ZERO_LS, ["zero", "nomatch"]]),
    ("zero-cmd-images", "tracks-art", {}, _zero("{{fields: [images], auto: no}}"),
     [_IMP, ["zero", "title:One"]]),
]

# ftintitle's command and `album_artist_no_feat`.
_FT_LS = ["ls", "-f", "$title|$artist|$artist_sort|$album_artist_no_feat"]
_FT_SETUP = [_IMP, ["modify", "-y", "artist=Art Artist feat. Bob", "title:One"],
             ["modify", "-y", "artist=Bob & Art Artist", "artist_sort=Bob & Art Artist",
              "title=Two (Club Mix)", "title:Two"]]


def _ft(cfg=""):
    return "{{plugins: [ftintitle], import: {{write: yes}}" + cfg + "}}"


ART_CASES += [
    ("ftintitle-cmd", "tracks", {}, _ft(),
     [*_FT_SETUP, ["ftintitle"], _FT_LS, ["write", "-p"], ["ftintitle"]]),
    ("ftintitle-drop", "tracks", {}, _ft(), [*_FT_SETUP, ["ftintitle", "-d"], _FT_LS]),
    ("ftintitle-limit", "tracks", {}, _ft(),
     [*_FT_SETUP, ["ftintitle", "--limit=1"], _FT_LS, ["ftintitle", "-l", "1", "title:Two"],
      _FT_LS]),
    ("ftintitle-options", "tracks", {},
     _ft(", ftintitle: {{format: 'ft. {{0}}', keep_in_artist: yes, bracket_keywords: [club], "
         "custom_words: [x]}}"),
     [*_FT_SETUP, ["modify", "-y", "artist=Art Artist x Carol", "title:One"], ["ftintitle"],
      _FT_LS]),
    ("ftintitle-preserve", "tracks", {}, _ft(", ftintitle: {{preserve_album_artist: no}}"),
     [_IMP, ["modify", "-y", "-a", "albumartist=Art Artist ft. Dee"],
      ["modify", "-y", "artist=Art Artist ft. Dee"], ["ftintitle"], _FT_LS,
      ["ls", "-a", "-f", "$albumartist|$album_artist_no_feat"]]),
    ("ftintitle-keep-preserve", "tracks", {}, _ft(),
     [_IMP, ["modify", "-y", "-a", "albumartist=Art Artist ft. Dee"],
      ["modify", "-y", "artist=Art Artist ft. Dee"], ["ftintitle"], _FT_LS, ["fields"]]),
    ("ftintitle-no-write", "tracks", {}, "{{plugins: [ftintitle]}}",
     [*_FT_SETUP, ["ftintitle"], ["write", "-p"]]),
]

# scrub: on import (beets' import_task_files, when tags are written) and its
# command; the files' tags as `write -p` shows them against the library.
_SCRUB = "{{plugins: [scrub], import: {{write: yes}}}}"
ART_CASES += [
    ("scrub-import", "tracks-art", {}, _SCRUB,
     [_IMP, ["write", "-p"], ["scrub", "-W", "nomatch"]]),
    ("scrub-import-nowrite", "tracks-art", {}, "{{plugins: [scrub]}}", [_IMP, ["write", "-p"]]),
    ("scrub-import-W", "tracks-art", {}, _SCRUB, [["import", "-A", "-q", "-W", "{music}"]]),
    ("scrub-auto-off", "tracks-art", {}, "{{plugins: [scrub], import: {{write: yes}}, "
     "scrub: {{auto: no}}}}", [_IMP]),
    ("scrub-cmd-W", "tracks-art", {}, "{{plugins: [scrub]}}",
     [_IMP, ["scrub", "-W", "title:One"], ["write", "-p"]]),
    ("scrub-cmd", "tracks-art", {}, "{{plugins: [scrub]}}",
     [_IMP, ["modify", "-y", "-W", "title=New"], ["scrub", "--limit=1"], ["write", "-p"],
      ["scrub", "--nowrite", "-l", "1"], ["scrub"], ["write", "-p"]]),
    ("scrub-zero", "tracks-art", {}, "{{plugins: [scrub, zero], zero: {{fields: [title]}}}}",
     [_IMP, ["scrub", "title:Two"], ["write", "-p"]]),
]

# edit: the command, with the scripted editor (rules: see editor.py).
def _edit(rules, extra=""):
    cfg = f"plugins: [edit], editor: {json.dumps(EDITOR)}, x_editor: {json.dumps(rules)}{extra}"
    return "{{" + cfg.replace("{", "{{").replace("}", "}}") + "}}"


_ED_LS = ["ls", "-f", "$id|$track|$title|$artist|$album|$year|$foo|$genres"]
_ED_WRITE = ", import: {write: yes}"
ART_CASES += [
    ("edit-cancel", "tracks", {}, _edit([[["One", "Uno"]]]), [_IMP, (["edit"], "c\n"), _ED_LS]),
    ("edit-apply", "tracks", {}, _edit([[["One", "Uno"], ["Art Album", "X"]]], _ED_WRITE),
     [_IMP, (["edit"], "a\n"), _ED_LS, ["ls", "-a", "-f", "$album"], ["write", "-p"]]),
    ("edit-default", "tracks", {}, _edit([[["One", "Uno"]]]), [_IMP, (["edit"], "\n"), _ED_LS]),
    ("edit-noop", "tracks", {}, _edit([None]), [_IMP, (["edit"], ""), _ED_LS]),
    ("edit-keep-editing", "tracks", {}, _edit([[["One", "Uno"]], [["Uno", "Eins"]]]),
     [_IMP, (["edit"], "e\na\n"), _ED_LS]),
    ("edit-keep-cancel", "tracks", {}, _edit([[["One", "Uno"]], [["Two", "Zwei"]]]),
     [_IMP, (["edit"], "e\nc\n"), _ED_LS]),
    ("edit-all", "tracks", {}, _edit([None, None]),
     [_IMP, ["modify", "-y", "genres=Rock; Pop", "comp=1", "rg_track_gain=-6.5", "foo=1",
             "title:One"], ["modify", "-y", "-a", "data_source=X"],
      ["edit", "--all", "title:One"], ["edit", "-a", "--all"]]),
    ("edit-album", "tracks", {}, _edit([[["Art Album", "New Album"]]], _ED_WRITE),
     [_IMP, (["edit", "-a"], "a\n"), ["ls", "-a", "-f", "$album|$albumartist"], _ED_LS]),
    ("edit-fields", "tracks", {}, _edit([[["year: 2001", "year: 1999"],
                                        ["genres: []", "genres:\n- Jazz\n- Funk"]]]),
     [_IMP, (["edit", "-f", "year", "--field", "genres", "title:One"], "a\n"), _ED_LS]),
    ("edit-add-flex", "tracks", {}, _edit([[["id: 1\n", "id: 1\nfoo: bar\nbaz: 7\n"]]]),
     [_IMP, (["edit"], "a\n"), _ED_LS, ["ls", "-f", "$baz"]]),
    ("edit-types", "tracks", {}, _edit([[["track: 1\n", "track: '7'\n"],
                                        ["title: One", "title: yes"],
                                        ["track: 2\n", "track: true\n"],
                                        ["title: Two", "title: 2020-01-02"]]]),
     [_IMP, (["edit"], "a\n"), _ED_LS]),
    ("edit-types-2", "tracks", {}, _edit([[["album: Art Album\n", "album: [a, b]\n"],
                                          ["artist: Art Artist\n", "artist: 0x1F\n"],
                                          ["title: Two", "title: ~"]]]),
     [_IMP, (["edit"], "a\n"), _ED_LS]),
    ("edit-malformed", "tracks", {}, _edit(["!MALFORMED", "id: 1\ntitle: Fixed\n"]),
     [_IMP, (["edit"], "n\n"), (["edit"], "y\na\n"), _ED_LS]),
    ("edit-parse-errors", "tracks", {}, _edit(["a: [1, 2\n", "a: b: c\n", "- 5\n", "x: -\n"]),
     [_IMP, (["edit"], "y\ny\ny\nn\n")]),
    ("edit-invalid", "tracks", {}, _edit(["wellformed: yes, but invalid"]),
     [_IMP, (["edit"], ""), _ED_LS]),
    ("edit-ids", "tracks", {}, _edit([[["id: 2", "id: 1"]], [["id: 2", "id: 9"]]]),
     [_IMP, (["edit"], ""), (["edit"], ""), _ED_LS]),
    ("edit-path", "tracks", {}, _edit([[["path: ", "path: /x"], ["title: One", "title: P"]]]),
     [_IMP, (["edit", "--all", "title:One"], "")]),
    ("edit-nothing", "tracks", {}, _edit([]), [_IMP, ["edit", "nomatch"]]),
    ("edit-no-editor", "tracks", {}, "{{plugins: [edit], editor: nonexistent-editor}}",
     [_IMP, ["edit"]]),
]

_INLINE_CRASH = ("an inline field's runtime error: beets crashes with a traceback "
                 "(InlineError), beets-rs prints the same InlineError message and exits 1")
_INLINE_UNSUPPORTED = ("Python beyond beets-rs' inline subset (lambda, import): beets "
                       "evaluates it, beets-rs stops with \"error in inline path field code: "
                       "... NotImplementedError: beets-rs' inline doesn't support ...\"")
_INLINE_SYNTAX = ("an inline definition that isn't Python: beets logs \"syntax error in "
                  "inline field definition\" with a Python traceback, beets-rs with the "
                  "parser's message")
EXPECTED.update({
    "inline error boom": _INLINE_CRASH,
    "inline error noname": _INLINE_CRASH,
    "inline error lam": _INLINE_UNSUPPORTED,
    "inline error imp": _INLINE_UNSUPPORTED,
    "inline ok field with syntax error": _INLINE_SYNTAX,
})

# albumtypes: $atypes (album types set with modify).
_AT = "{{plugins: [albumtypes]}}"
ART_CASES += [
    ("albumtypes", "tracks", {}, _AT,
     [_IMP, ["modify", "-y", "-a", "albumtypes=ep; live; soundtrack; remix"],
      ["ls", "-a", "-f", "$albumtypes|$atypes"], ["ls", "-f", "$title $atypes"],
      ["modify", "-y", "-a", "mb_albumartistid=89ad4ac3-39f7-470e-963a-56509c546377",
       "albumtypes=compilation; live"], ["ls", "-a", "-f", "$atypes"], ["fields"]]),
    ("albumtypes-config", "tracks", {},
     "{{plugins: [albumtypes], albumtypes: {{types: [[live, LIVE], [ep, ''], {{remix: RMX}}], "
     "ignore_va: [], bracket: '<>'}}}}",
     [_IMP, ["modify", "-y", "-a", "albumtypes=ep; live; remix"], ["ls", "-a", "-f", "$atypes"]]),
    ("albumtypes-no-bracket", "tracks", {},
     "{{plugins: [albumtypes], albumtypes: {{bracket: ''}}}}",
     [_IMP, ["modify", "-y", "-a", "albumtypes=ep; single"], ["ls", "-a", "-f", "$atypes"]]),
]

_PLUGIN_LOAD = ("a plugin that fails to set up (bad `fetchart.sources`): beets logs \"** error "
                "loading plugin fetchart\" with a Python traceback and runs without it; beets-rs "
                "logs the same line with the error message (and the `fetchart` command fails "
                "with it)")
_EXTRACT_CRASH = ("extractart -o into a missing directory: beets crashes with a traceback "
                  "(FileNotFoundError), beets-rs reports the same error and exits 1")
EXPECTED.update({
    "art:sources-invalid:1": _PLUGIN_LOAD,
    "art:sources-invalid:state": _PLUGIN_LOAD,
    "art:sources-empty:1": _PLUGIN_LOAD,
    "art:sources-empty:state": _PLUGIN_LOAD,
    "art:extractart:2": _EXTRACT_CRASH,
})

# replaygain, ffmpeg backend (`rg/`: an MP3 album on two discs, an Opus album,
# a FLAC album): gains computed on import and by the command, as stored
# (`ls`) and as written to the files (`info`).
_RG_LS = ["ls", "-f", "$title|$disc|$rg_track_gain|$rg_track_peak|$rg_album_gain|$rg_album_peak"
          "|$r128_track_gain|$r128_album_gain"]
_RG_DB = ["ls", "-f", "$title %ifdef{{rg_track_gain,$rg_track_gain,-}} "
          "%ifdef{{rg_album_peak,$rg_album_peak,-}} %ifdef{{r128_album_gain,$r128_album_gain,-}}"]
_RG_TAGS = ["-p", "info", "info", "-i",
            "title,rg_track_gain,rg_track_peak,rg_album_gain,rg_album_peak,r128_track_gain,"
            "r128_album_gain", "artist:RG Artist"]


def _rg(cfg=""):
    return "{{plugins: [replaygain], replaygain: {{backend: ffmpeg" + cfg + "}}}}"


ART_CASES += [
    ("rg-import", "rg", {}, _rg(), [_IMP, _RG_LS, _RG_DB, _RG_TAGS]),
    ("rg-import-write", "rg", {}, "{{import: {{write: yes}}, " + _rg()[2:],
     [_IMP, _RG_TAGS]),
    ("rg-import-singletons", "rg", {}, _rg(), [["import", "-A", "-q", "-s", "{music}"], _RG_LS]),
    ("rg-import-parallel", "rg", {}, _rg(", parallel_on_import: yes, threads: 2"),
     [_IMP, _RG_LS]),
    ("rg-auto-off", "rg", {}, _rg(", auto: no"), [_IMP, _RG_LS, ["replaygain", "-a"], _RG_LS]),
    ("rg-cmd", "rg", {}, _rg(", auto: no"),
     [_IMP, ["replaygain"], _RG_LS, ["replaygain"], ["replaygain", "-a", "album:RG Album"],
      _RG_LS, ["replaygain", "-a"], ["replaygain", "-f", "-t", "0", "title:One"],
      ["replaygain", "-a", "-l", "1", "-f"], _RG_DB, _RG_TAGS]),
    ("rg-cmd-write", "rg", {}, _rg(", auto: no"),
     [_IMP, ["replaygain", "-a", "-w", "-t", "1"], _RG_TAGS, ["write", "-p"]]),
    ("rg-cmd-nowrite", "rg", {}, "{{import: {{write: yes}}, " + _rg(", auto: no")[2:],
     [_IMP, ["replaygain", "-W"], _RG_TAGS, ["replaygain", "-a"], _RG_TAGS]),
    ("rg-targetlevel", "rg", {}, _rg(", targetlevel: 84, r128_targetlevel: 89, peak: sample"),
     [_IMP, _RG_LS]),
    ("rg-per-disc", "rg", {}, _rg(", per_disc: yes"), [_IMP, _RG_LS]),
    ("rg-r128-formats", "rg", {}, _rg(", r128: [MP3, FLAC]"), [_IMP, _RG_LS, _RG_TAGS]),
    ("rg-no-r128", "rg", {}, _rg(", r128: []"), [_IMP, _RG_LS]),
    ("rg-overwrite", "rg", {}, _rg(", overwrite: yes, auto: no"),
     [_IMP, ["replaygain", "-a"], ["modify", "-y", "rg_track_gain=1.5", "title:One"],
      ["replaygain", "-a"], ["replaygain", "-a", "-f"], _RG_LS]),
    ("rg-mixed-album", "rg", {}, _rg(", auto: no"),
     [["import", "-A", "-q", "--flat", "{music}"], ["replaygain", "-a"], _RG_LS]),
    ("rg-bad-backend", "rg", {}, "{{plugins: [replaygain], replaygain: {{backend: bogus}}}}",
     [["replaygain"]]),
    ("rg-bad-peak", "rg", {}, _rg(", peak: loud"), [["replaygain"]]),
    ("rg-gstreamer", "rg", {}, "{{plugins: [replaygain], replaygain: {{backend: gstreamer}}}}",
     [["replaygain"]]),
    ("rg-command-missing", "rg", {}, "{{plugins: [replaygain]}}", [["replaygain"]]),
]
EXPECTED.update({
    "art:rg-bad-backend:1": _PLUGIN_LOAD,
    "art:rg-bad-peak:1": _PLUGIN_LOAD,
    "art:rg-gstreamer:1": _PLUGIN_LOAD,
    "art:rg-command-missing:1": _PLUGIN_LOAD,
    "art:rg-bad-backend:state": _PLUGIN_LOAD,
    "art:rg-bad-peak:state": _PLUGIN_LOAD,
    "art:rg-gstreamer:state": _PLUGIN_LOAD,
    "art:rg-command-missing:state": _PLUGIN_LOAD,
})

# ---------------------------------------------------------------------------
# duplicates plugin. Read-only listings on the shared corpus (main phase);
# the cases that change the library run in the art phase, each on a fresh
# library with the same two tracks imported twice.

DUP_ARGS = [
    ["dup"],
    ["dup", "-a"],
    ["dup", "-k", "album", "-F", "-c"],
    ["duplicates", "-a", "-k", "album", "-F", "-c"],
    ["dup", "-k", "artist", "-k", "album", "-s"],
    ["dup", "-k", "album", "-k", "nonexistent", "-F", "-c"],
    ["dup", "-k", "album", "-k", "nonexistent", "-s"],
    ["dup", "-k", "album", "-p"],
    ["dup", "-a", "-k", "albumartist", "-F", "-f", "$albumartist|$album|$id"],
    ["dup", "-k", "album", "-l", "5", "-F"],
    ["dup", "-k", "album", "-F", "Hit"],
    ["dup", "-k", "year", "-F", "-c"],
    ["dup", "-k", "length", "-F"],
    ["dup", "-k", "comp", "-k", "singleton", "-F", "-c"],
    ["dup", "-a", "-k", "albumtype", "-k", "year", "-p", "-c"],
]


_DUP_CFG = "{{plugins: [duplicates], import: {{duplicate_action: keep}}}}"
_DUP_IMP = [_IMP, _IMP]
_DUP_LS = ["ls", "-f", "$id $album_id $title $grouping $mb_trackid $dupe"]
_DUP_LSA = ["ls", "-a", "-f", "$id $album $dupe"]
_CKSUM = "sh -c 'cksum < \"$0\"' {{file}}"
ART_CASES += [
    ("dup-list", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-F", "-c"], ["dup", "-a", "-k", "album", "-c"],
      ["dup", "-k", "title", "-p"], ["dup", "-a", "-k", "album", "-F", "-p"]]),
    ("dup-tag", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-t", "dupe=1"], _DUP_LS]),
    ("dup-tag-album", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-a", "-k", "album", "-t", "dupe=1"], _DUP_LS, _DUP_LSA]),
    ("dup-tag-bad", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-t", "a=b=c"], _DUP_LS]),
    ("dup-remove", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-r"], _DUP_LS, _DUP_LSA]),
    ("dup-delete", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-d"], _DUP_LS, _DUP_LSA]),
    ("dup-delete-album", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-a", "-k", "album", "-d"], _DUP_LS, _DUP_LSA]),
    ("dup-copy", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-k", "title", "-o", "{root}/out"], ["ls", "-p"]]),
    ("dup-move-album", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-a", "-k", "album", "-m", "{root}/out"], ["ls", "-p"]]),
    ("dup-merge-items", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["modify", "-y", "-M", "id:3", "grouping=G"],
      ["dup", "-k", "title", "-M", "-F", "-f", "$id $grouping"], _DUP_LS]),
    ("dup-merge-albums", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["modify", "-y", "-M", "id:3", "mb_trackid=x3"],
      ["dup", "-a", "-k", "album", "-M", "-F"], _DUP_LS, ["ls", "-f", "$id $album_id $path"]]),
    ("dup-checksum", "tracks", {}, _DUP_CFG,
     [*_DUP_IMP, ["dup", "-C", _CKSUM, "-F"], ["dup", "-C", _CKSUM, "-c"],
      ["ls", "-f", "$id $sh"], ["dup", "-C", "false {{file}}", "-F"]]),
    ("dup-tiebreak", "tracks", {},
     "{{plugins: [duplicates], import: {{duplicate_action: keep}}, "
     "duplicates: {{tiebreak: {{items: [id]}}}}}}",
     [*_DUP_IMP, ["dup", "-k", "title", "-F", "-f", "$id $title"]]),
]


# ---------------------------------------------------------------------------
# types plugin (flexible attribute types) and queries on album getters
# (`albumtotal`, the missing plugin's `$missing`). Read-only on the shared
# corpus; `modify` cases run in the art phase on fresh libraries.

EXTRA_CONFIGS["types.yaml"] = (
    "types: {{rating: int, Flex_Attr: bool, albummood: int, mood: float}}\n")
EXTRA_CONFIGS["typesf.yaml"] = "types: {{rating: float}}\n"
EXTRA_CONFIGS["typesd.yaml"] = "time_format: '%Y-%m-%d'\ntypes: {{releasedate: date}}\n"
_TY = ["-c", f"{ROOT}/types.yaml", "-p", "types"]
_TYF = ["-c", f"{ROOT}/typesf.yaml", "-p", "types"]
_TYD = ["-c", f"{ROOT}/typesd.yaml", "-p", "types"]
_MISS = ["-p", "missing"]
TYPES_ARGS = [
    [*_TY, "ls", "-f", "$title|$rating|$Flex_Attr|$mood|%ifdef{rating,Y,N}", "rating:1.."],
    [*_TY, "ls", "-f", "$title $rating", "rating+", "title+"],
    [*_TY, "ls", "-f", "$title $rating", "rating-", "title+"],
    [*_TY, "ls", "-f", "$title", "rating:5..7"],
    [*_TY, "ls", "-f", "$title", "rating:10"],
    [*_TY, "ls", "-f", "$title", "rating:=5"],
    [*_TY, "ls", "-f", "$title", "rating::^1"],
    [*_TY, "ls", "-f", "$title", "rating:1"],
    [*_TY, "ls", "-f", "$title", "rating:abc"],
    [*_TY, "ls", "-f", "$title", "Flex_Attr:false"],
    [*_TY, "ls", "-f", "$title", "mood:..1"],
    [*_TY, "ls", "-a", "-f", "$album $albummood", "albummood:0"],
    [*_TY, "ls", "-a", "-f", "$album", "albummood+", "album+"],
    [*_TY, "ls", "-f", "$title $albummood", "albummood:0"],
    ["-c", f"{ROOT}/types.yaml", "-p", "types,info", "info", "-l", "-i",
     "rating,Flex_Attr,nope", "rating:1.."],
    [*_TYF, "ls", "-f", "$title $rating", "rating:3..7.5"],
    [*_TYF, "ls", "-f", "$title $rating", "rating+", "title+"],
    [*_TYD, "ls", "-f", "$title $releasedate", "releasedate:2001"],
    [*_TYD, "ls", "-f", "$title $releasedate", "releasedate:..2002-01"],
    [*_MISS, "ls", "-a", "-f", "$album $missing", "missing:1.."],
    [*_MISS, "ls", "-a", "-f", "$album", "missing:0"],
    [*_MISS, "ls", "-a", "-f", "$album", "missing:=-1"],
    [*_MISS, "ls", "-a", "-f", "$album $missing", "missing+", "album+"],
    [*_MISS, "ls", "-f", "$title", "missing:-1"],
    ["ls", "-a", "-f", "$album $albumtotal", "albumtotal:3"],
    ["ls", "-a", "-f", "$album", "albumtotal:2..3"],
    ["ls", "-a", "-f", "$album $albumtotal", "albumtotal-", "album+"],
    ["ls", "-f", "$title", "albumtotal:5"],
    ["ls", "-a", "-f", "$album", "missing:1"],
]

_TY_CFG = "{{plugins: [types], types: {{myint: int, myfloat: float, mybool: bool, mydate: date}}}}"
_TY_LS = ["ls", "-f", "$title|$myint|$myfloat|$mybool|$mydate|%ifdef{{myint,Y,N}}"]
ART_CASES += [
    ("types-modify", "tracks", {}, _TY_CFG,
     [_IMP, ["modify", "-y", "title:One", "myint=2", "myfloat=3.14159", "mybool=yes",
             "mydate=2020-01-02 10:11:12"],
      ["modify", "-y", "title:Two", "myint=abc", "myfloat=-9.1", "mybool=xyz",
       "mydate=garbage"],
      _TY_LS, ["ls", "-f", "$title", "myint:1..3"], ["ls", "-f", "$title", "mybool:false"],
      ["ls", "-f", "$title", "mydate:2020"], ["ls", "-f", "$title", "myfloat+"],
      ["modify", "-y", "title:One", "myint=3", "mybool=no"],
      ["modify", "-y", "title:One", "myint!"], _TY_LS]),
    ("types-album", "tracks", {}, _TY_CFG,
     [_IMP, ["modify", "-y", "-a", "album:Art", "myint=5", "mybool=1"],
      ["ls", "-a", "-f", "$album $myint $mybool", "myint:4..6"],
      ["ls", "-f", "$title $myint", "myint:5"]]),
    ("types-time-format", "tracks", {},
     "{{plugins: [types], time_format: '%Y-%m-%d', types: {{mydate: date}}}}",
     [_IMP, ["modify", "-y", "title:One", "mydate=1999-01-01"],
      ["ls", "-f", "$title $mydate", "mydate:..1999-07"]]),
]

# convert: real ffmpeg on `conv` (a FLAC and an Ogg track); state: the
# converted files' format, tags, embedded art and link targets, playlists.
_CV_CFG = ("{{plugins: [convert], convert: {{dest: '{root}/out', threads: 1, "
           "tmpdir: '{root}'" + "{extra}}}}}")
_CV_LS = ["ls", "-f", "$path $format"]


def _cv(extra=""):
    return _CV_CFG.replace("{extra}", (", " + extra) if extra else "")


_CV_IMP = ["import", "-A", "-q", "{music}"]
# One file per import: beets' import pipeline prints every task's path
# before the stages' messages.
_CV_SINGLES = [["import", "-A", "-q", "-s", "{music}/01.flac"],
               ["import", "-A", "-q", "-s", "{music}/02.ogg"]]
ART_CASES += [
    ("convert-basic", "conv", {}, _cv(),
     [_CV_IMP, (["convert"], "n\n"), (["convert"], "y\n"), ["convert", "-y"], _CV_LS]),
    ("convert-pretend-playlist", "conv", {}, _cv(),
     [_CV_IMP, ["convert", "-p", "-m", "pl.m3u"], ["convert", "-y", "-m", "sub/pl.m3u8"]]),
    ("convert-format-link", "conv", {}, _cv(),
     [_CV_IMP, ["convert", "-y", "-f", "ogg", "-l"], ["convert", "-y", "-f", "vorbis",
                                                      "-d", "{root}/out/v", "title:Dos"]]),
    ("convert-hardlink", "conv", {}, _cv(),
     [_CV_IMP, ["convert", "-y", "-f", "FLAC", "-H", "-l"]]),
    ("convert-keep-new", "conv", {}, _cv(),
     [_CV_IMP, ["convert", "-y", "-k", "-f", "ogg"], _CV_LS,
      ["convert", "-y", "-k", "-p", "-f", "opus"]]),
    ("convert-album-art", "conv", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, convert], convert: {{dest: '{root}/out', threads: 1, "
     "copy_album_art: yes, album_art_maxwidth: 100}}}}",
     [_CV_IMP, ["convert", "-y", "-a", "-p"], ["convert", "-y", "-a"], ["convert", "-y", "-a"]]),
    ("convert-album-art-copy", "conv", {"cover.jpg": "small.jpg"},
     "{{plugins: [fetchart, convert], art_filename: 'folder', convert: {{dest: '{root}/out', "
     "threads: 1, copy_album_art: yes, embed: no, album_art_maxwidth: 100}}}}",
     [_CV_IMP, ["convert", "-y", "-a", "-p", "-l"], ["convert", "-y", "-a", "-l"]]),
    ("convert-no-convert", "conv", {}, _cv("no_convert: 'format:OGG , title:Uno'"),
     [_CV_IMP, ["convert", "-y"], ["convert", "-y", "-F", "-d", "{root}/out/f"]]),
    ("convert-never-lossy", "conv", {}, _cv("never_convert_lossy_files: yes, format: opus"),
     [_CV_IMP, ["convert", "-y"]]),
    ("convert-max-bitrate", "conv", {}, _cv("max_bitrate: 1, format: ogg"),
     [_CV_IMP, ["convert", "-y", "-m", "pl.m3u", "--limit", "1"], ["convert", "-y"]]),
    ("convert-paths", "conv", {},
     _cv("paths: {{default: '$album/$track-$title', 'format:OGG': 'ogg/$title'}}"),
     [_CV_IMP, ["convert", "-y", "-m", "lists/all.m3u"]]),
    ("convert-command-shortcut", "conv", {},
     _cv("command: 'ffmpeg -i $source -y -vn -acodec libvorbis \"$dest\"', extension: oga"),
     [_CV_IMP, ["convert", "-y", "-f", "flac"]]),
    # 5.23 c/f: `after_convert` reaches a `hook` listener from a convert
    # worker thread (its command's output is part of the step's stdout).
    ("convert-after-convert-hook", "conv", {},
     "{{plugins: [convert, hook], hook: {{hooks: [{{event: after_convert,"
     " command: 'echo AFTER {{item.title}} {{keepnew}}'}}]}}, convert:"
     " {{dest: '{root}/out', threads: 2, tmpdir: '{root}'}}}}",
     [_CV_IMP, ["convert", "-y", "-f", "ogg", "title:Uno"], _CV_LS]),
    ("convert-errors", "conv", {},
     _cv("formats: {{bad: {{extension: x}}, fail: {{command: 'false $source $dest', "
         "extension: fl}}, gone: 'no-such-encoder-xyz $source $dest'}}"),
     [_CV_IMP, ["convert", "-y", "-f", "xyz"], ["convert", "-y", "-f", "bad"],
      ["convert", "-y", "-f", "fail"], ["convert", "-y", "-f", "gone", "title:Uno"],
      ["convert", "-y", "nomatch"], ["convert", "-y", "-t", "0"]]),
    ("convert-no-dest", "conv", {}, "{{plugins: [convert]}}",
     [_CV_IMP, ["convert", "-y"], ["convert", "-p", "nomatch"]]),
    ("convert-write-metadata-off", "conv", {}, _cv("write_metadata: no, id3v23: yes"),
     [_CV_IMP, ["modify", "-y", "title=Changed"], ["convert", "-y"]]),
    ("convert-id3v23", "conv", {}, _cv("id3v23: yes"),
     [_CV_IMP, ["modify", "-y", "title:Dos", "title=Changed"], ["convert", "-y"]]),
    # On import (singletons: beets converts an album's items in parallel,
    # so their messages come in any order).
    ("convert-auto", "conv", {}, _cv("auto: yes"), [*_CV_SINGLES, _CV_LS]),
    ("convert-auto-delete", "conv", {}, _cv("auto: yes, delete_originals: yes, format: ogg"),
     [*_CV_SINGLES, _CV_LS]),
    ("convert-auto-move", "conv", {},
     "{{plugins: [convert], import: {{copy: no, move: yes}}, convert: {{dest: '{root}/out', "
     "threads: 1, auto: yes, quiet: yes, tmpdir: '{root}'}}}}", [*_CV_SINGLES, _CV_LS]),
    ("convert-auto-fail", "conv", {}, _cv("auto: yes, command: 'false'"), [*_CV_SINGLES, _CV_LS]),
    ("convert-auto-keep", "conv", {}, _cv("auto_keep: yes"), [*_CV_SINGLES, _CV_LS]),
]
