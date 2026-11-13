"""Diffbeets cases: imports."""

ORDER = 30

import json
import os
import sys

# ---------------------------------------------------------------------------
# Import phase: both tools import against a fake MusicBrainz serving
# tests/fixtures/mb (see importphase.py).

_FX = {"artist": "Fixture Artist", "album": "Fixture Album"}
_FX_ID = "11111111-1111-1111-1111-111111111111"

# Directories of generated MP3s (1 second each): name -> [(file, tags)].
IMPORT_CORPUS = {
    # Tagged like the fixture release, without ids.
    "fx": [(f"{n}.mp3", {**_FX, "title": t, "track": str(n)})
           for n, t in enumerate(["Alpha", "Beta", "Gamma"], 1)],
    # With the release id, disc numbers and the pregap track.
    "tagged": [(f"{n}.mp3", {**_FX, "album_artist": "Fixture Artist", "title": t,
                             "track": str(n), "disc": "1", "date": "2004",
                             "MusicBrainz Album Id": _FX_ID})
               for n, t in enumerate(["Hidden Intro", "Alpha", "Beta", "Gamma"])],
    # The VA compilation on two titled discs (release 3333...).
    "sampler": [(f"{n}.mp3", {"album": "Sampler of Sounds", "album_artist": "Various Artists",
                             "artist": a, "title": t, "track": str(tr), "disc": str(d),
                             "date": "2010", "publisher": "Sampler Records",
                             "CATALOGNUMBER": "SMP 001", "BARCODE": "0042"})
                for n, (d, tr, a, t) in enumerate([
                    (1, 1, "Artist A feat. Artist B", "Ocean Song"),
                    (1, 2, "The Band", "Night Drive"),
                    (2, 1, "Artist A & Artist C", "Ocean Song (Live)"),
                    (2, 2, "Artist B", "Final Call (edit)")], 1)],
    # The multi-artist EP (release 4444...).
    "duets": [(f"{n}.mp3", {"album": "Duets Collection", "artist": "Artist A & Artist B vs. C",
                           "title": t, "track": str(n)})
              for n, t in enumerate(["First Duet", "Second Duet", "Third Duet"], 1)],
    # Spare files the rescan cases put into a directory at the prompt.
    "extra": [("delta.mp3", {**_FX, "title": "Delta", "track": "4"})],
    # Two same-named albums under one import root: %aunique sees both in
    # beets' threaded pipeline (both get a disambiguator); sequential sees
    # only the first while placing it.
    "twin": [("a/01.mp3", {**_FX, "title": "Alpha", "track": "1"}),
             ("b/01.mp3", {**_FX, "title": "Beta", "track": "1"})],
    # fromfilename: no titles or track numbers in the tags, only in the
    # file names.
    "notitle": [(f"{n:02d} - Fixture Artist - {t}.mp3", {"album": "Fixture Album"})
                for n, t in enumerate(["Alpha", "Beta", "Gamma"], 1)],
}

# The scripted editor (`editor` option) for the edit cases.
EDITOR = f"{sys.executable} -I {os.path.join(os.path.dirname(os.path.abspath(__file__)), 'editor.py')}"


def _import_edit(rules, extra=""):
    """An import case's config for the edit plugin with these editor rules."""
    return (f"plugins: [musicbrainz, edit]\neditor: {json.dumps(EDITOR)}\n"
            f"x_editor: {json.dumps(rules)}\n{extra}")


# (name, corpus directory, import args, extra config, answers). An answer is
# typed at the next prompt; a tuple acts on the music first: ("remove",
# file), ("empty",), ("copy", import-src path, new name).
IMPORT_CASES = [
    # Interactive (-t) imports with the match options of autotag.rs.
    ("t", "fx", ["-t", "-C"], "", ["1", "a"]),
    ("t-id-match", "tagged", ["-t", "-C"], "", ["a"]),
    ("t-ignored", "fx", ["-t", "-C"], "match:\n    ignored: missing_tracks\n", ["u"]),
    ("t-required", "fx", ["-t", "-C"], "match:\n    required: label\n", ["a"]),
    ("t-preferred", "fx", ["-t", "-C"],
     "match:\n    preferred:\n        media: ['Digital Media', CD]\n"
     "        countries: [GB]\n        original_year: yes\n", ["a"]),
    ("t-ignored-media", "tagged", ["-t", "-C"],
     "match:\n    ignored_media: [DVD-Video]\n    ignore_data_tracks: no\n", ["a"]),
    ("t-disambig-fields", "fx", ["-t", "-C"],
     "match:\n    album_disambig_fields: albumtype releasegroupdisambig barcode"
     " script language mediums bogus\n", ["2", "a"]),
    ("t-weights", "fx", ["-t", "-C"],
     "match:\n    distance_weights:\n        album: 0.5\n        missing_tracks: 0.0\n"
     "    track_length_grace: 1000\n    max_rec:\n        tracks: low\n", ["a"]),
    # Other modes.
    ("q", "fx", ["-q", "-C"], "", []),
    ("q-asis", "fx", ["-q", "-C", "--quiet-fallback=asis"], "", []),
    ("q-id-match", "tagged", ["-q", "-C"],
     "match:\n    strong_rec_thresh: 0.2\n    max_rec:\n        missing_tracks: strong\n", []),
    ("s", "fx", ["-t", "-s", "-C"], "", ["1", "a", "u", "a"]),
    ("A", "fx", ["-A", "-C"], "", []),
    # %aunique across two same-named albums imported in one run, sequential
    # (the threaded case is scheduling-dependent in beets: see
    # importer::tests::threaded_import_disambiguates_same_named_albums).
    ("aunique-sequential", "twin", ["-A", "-q"],
     "threaded: no\nimport:\n    duplicate_action: keep\n", []),
    ("t-abort", "fx", ["-t", "-C"], "", ["b"]),
    # "Rescan directory": a file added, one removed, the directory emptied.
    ("rescan-add", "fx", ["-t", "-C"], "",
     [("copy", "extra/delta.mp3", "4.mp3"), "r", "a"]),
    ("rescan-remove", "tagged", ["-t", "-C"], "", [("remove", "0.mp3"), "r", "a"]),
    ("rescan-empty", "fx", ["-t", "-C"], "", [("empty",), "r"]),
    ("rescan-group", "fx", ["-t", "-C"], "", ["g", "u"]),
    # Metadata fidelity: multi-artist credits, VA, titled discs, secondary
    # types, works and relations, and the options shaping applied metadata.
    ("t-va", "sampler", ["-t", "-C"], "", ["a"]),
    ("t-va-options", "sampler", ["-t", "-C"],
     "per_disc_numbering: yes\nartist_credit: yes\noriginal_date: yes\nva_name: VA\n"
     "musicbrainz:\n    genres: yes\n    search_limit: 3\n"
     "    external_ids:\n        discogs: yes\n        bandcamp: yes\n"
     "    extra_tags: [year, catalognum, country, media, label, barcode, tracks, alias]\n",
     ["a"]),
    ("t-va-tags", "sampler", ["-t", "-C"],
     "musicbrainz:\n    genres: yes\n    genres_tag: tag\n"
     "overwrite_null:\n    album: [asin, label]\n    track: [arrangers, work, bpm]\n",
     ["a"]),
    ("t-duets", "duets", ["-t", "-C"], "", ["a"]),
    ("t-duets-credit", "duets", ["-t", "-C"],
     "artist_credit: yes\nimport:\n    languages: [de]\n", ["a"]),
    ("s-va", "sampler", ["-t", "-s", "-C"], "", ["a", "a", "a", "a"]),
    # zero: on write while importing, and its as-is warning.
    ("zero-apply", "tagged", ["-t", "-C"],
     "plugins: [musicbrainz, zero]\nimport:\n    write: yes\n"
     "zero:\n    fields: [title, label, catalognum]\n    title: ['^gam']\n"
     "    update_database: yes\n", ["a"]),
    ("zero-asis", "fx", ["-t", "-C"],
     "plugins: [musicbrainz, zero]\nimport:\n    write: yes\nzero:\n    fields: title\n", ["u"]),
    # ftintitle: on fetched releases and recordings, and as an import stage
    # (before the files are placed, so the titles reach the paths).
    ("ftintitle-t", "sampler", ["-t"], "plugins: [musicbrainz, ftintitle]\n", ["a"]),
    ("ftintitle-s", "sampler", ["-t", "-s"], "plugins: [musicbrainz, ftintitle]\n",
     ["a", "a", "a", "a"]),
    ("ftintitle-A", "sampler", ["-A"],
     "plugins: [musicbrainz, ftintitle]\nimport:\n    write: yes\n", []),
    ("ftintitle-A-options", "sampler", ["-A"],
     "plugins: [musicbrainz, ftintitle]\nftintitle:\n    format: 'with {}'\n"
     "    keep_in_artist: yes\n", []),
    ("ftintitle-auto-off", "sampler", ["-t"],
     "plugins: [musicbrainz, ftintitle]\nftintitle:\n    auto: no\n", ["a"]),
    # edit: "eDit" and "edit Candidates" at the prompt (tools/diffbeets/editor.py
    # edits the YAML, the same way for both tools; see _import_edit below).
    *[(f"edit-{n}", music, args, _import_edit(rules, extra), answers)
      for n, music, args, rules, extra, answers in [
          ("d", "fx", ["-t", "-C"], [[["Alpha", "Alpha Edited"], ["Fixture Album", "Edited Album"]]],
           "", ["d", "a"]),
          ("d-cancel", "fx", ["-t", "-C"], [[["Alpha", "Alpha Edited"]]], "", ["d", "c", "u"]),
          ("d-keep", "fx", ["-t", "-C"], [[["Beta", "B1"]], [["B1", "B2"]]], "", ["d", "e", "a"]),
          ("d-noop", "fx", ["-t", "-C"], [None], "", ["d", "u"]),
          ("d-docs", "fx", ["-t", "-C"], ["album: x\n", [["x", "y"]]], "", ["d", "n", "u"]),
          ("c-list", "fx", ["-t", "-C"], [[["Alpha", "Alpha Edited"]]], "", ["c", "1", "a"]),
          ("c-back", "fx", ["-t", "-C"], [[["Alpha", "Alpha Edited"]]], "", ["c", "b", "u"]),
          ("c-shown", "fx", ["-t", "-C"], [[["Alpha", "Alpha Edited"]]], "", ["1", "c", "a"]),
          ("header-fields", "fx", ["-t", "-C"], [[["Fixture Album", "Hdr"]]],
           "edit:\n    albumfields: album track title\n    itemfields: title\n", ["d", "a"]),
          ("no-header", "fx", ["-t", "-C"], [[["Gamma", "G"]]],
           "edit:\n    albumfields: ''\n", ["d", "a"]),
          ("s", "fx", ["-t", "-s", "-C"], [[["Alpha", "Solo"]], [["Beta", "B"]]], "",
           ["d", "a", "c", "1", "a", "s"]),
      ]],
    # fromfilename (import_task_start: only while autotagging)
    ("fromfilename-t", "notitle", ["-t", "-C"], "plugins: [musicbrainz, fromfilename]\n",
     ["2", "a"]),
    ("fromfilename-s", "notitle", ["-t", "-s", "-C"], "plugins: [musicbrainz, fromfilename]\n",
     ["1", "a", "1", "a", "1", "a"]),
    ("fromfilename-A", "notitle", ["-A", "-C"], "plugins: [musicbrainz, fromfilename]\n", []),
    ("fromfilename-u", "notitle", ["-t", "-C"], "plugins: [musicbrainz, fromfilename]\n", ["u"]),
    ("zero-q-asis", "fx", ["-q", "-C", "--quiet-fallback=asis"],
     "plugins: [musicbrainz, zero]\nzero:\n    fields: title\n", []),
]
