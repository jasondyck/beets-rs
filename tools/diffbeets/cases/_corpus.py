"""Diffbeets cases:  corpus."""

ORDER = 10

"""Corpus, config and cases for diffbeets.py.

Case ids are "<kind>:<args>"; EXPECTED maps a case id to the reason the two
tools are allowed to differ there.
"""

import json
import os
import sys

# In case arguments, replaced by each tool's own config directory.
ROOT = "<<ROOT>>"

# ---------------------------------------------------------------------------
# Config shared by both tools ({root} is each tool's own config directory).

CONFIG = """\
directory: {root}/lib
library: {root}/library.db
plugins: []
import:
    copy: yes
    write: no
    resume: no
    incremental: no
    duplicate_action: keep
    log:
ui:
    color: no
paths:
    default: $albumartist/$album%aunique{{}}/$track $title
    singleton: Non-Album/$artist/$title%sunique{{}}
    comp: Compilations/$album%aunique{{}}/$track $title
    format:FLAC: Lossless/$albumartist/$album/$disc-$track $title
"""

# Extra configs layered with -c for mv -p cases.
EXTRA_CONFIGS = {
    "replace.yaml": """\
replace:
    '[aeiou]': '_'
    '^\\.': 'DOT'
    '\\s+$': ''
""",
    "asciify.yaml": "asciify_paths: yes\n",
    "sep.yaml": "path_sep_replace: '+'\n",
    "maxlen.yaml": "max_filename_length: 20\n",
    "empty.yaml": "empty_component_replace: EMPTY\n",
    "nocaseins.yaml": "sort_case_insensitive: no\n",
    "sortcfg.yaml": "sort_item: title- \nsort_album: year+ album-\n",
    "art.yaml": "art_filename: 'cover-$album'\n",
    "id3v23.yaml": "id3v23: yes\n",
    "unthreaded.yaml": "threaded: no\n",
    # the plugin
    "the.yaml": "plugins: [the]\n",
    "the-custom.yaml": "plugins: [the]\nthe:\n    a: no\n    patterns: ['^artist\\s', '^(hit)\\s+',"
                       " 'zoo$']\n    format: '{{1}} ({{0}})'\n",
    "the-strip.yaml": "plugins: [the]\nthe:\n    strip: yes\n    patterns: ['^artist\\s']\n",
    "the-none.yaml": "plugins: [the]\nthe:\n    the: no\n    a: no\n",
    # inline (braces doubled: these configs go through str.format)
    "inline.yaml": """\
plugins: [inline]
item_fields:
    initial: (albumartist or artist or '_')[0].upper() + u'.'
    disc_and_track: f"{{disc:02d}}.{{track:02d}}" if disctotal > 1 else f"{{track:02d}}"
    minsec: "'%d:%02d' % divmod(int(length), 60)"
    decade: f'{{year // 10 * 10}}s' if year else 'Unknown'
    kbps: f'{{bitrate // 1000}}kbps'
    genre1: genres[0] if genres else 'none'
    fname: path.decode('utf-8').split('/')[-1]
    shout: title.upper().replace(' ', '_')
    half: length / 2
    later: track > 1
    nothing: None
    glist: genres
    compf: comp
    single: db_obj.singleton
    words: len(title.split())
    padded: str(track).zfill(3) + '|' + title[:5].ljust(5, '.') + '|'
    kind: |
        if disctotal > 1:
            return 'multi'
        elif not track:
            return 'none'
        return 'single'
    sortname: |
        a = albumartist or artist
        if a.lower().startswith('the '):
            a = a[4:] + ', The'
        return a
album_fields:
    n: len(items)
    total_len: round(sum(i.length for i in items), 1)
    first: items[0].title if items else ''
    tracks: "','.join(str(t) for t in sorted(i.track for i in items))"
    alb_total: db_obj.albumtotal
    avg_br: |
        total = 0
        for item in items:
            total += item.bitrate
        return total // len(items)
paths:
    default: $initial/$sortname/$album%aunique{{}}/$disc_and_track $title
    singleton: Non-Album/$initial/$artist/$title
""",
    "inline-errors.yaml": """\
plugins: [inline]
item_fields:
    boom: 1 / (track - track)
    noname: undefined_thing
    lam: '(lambda x: x)(1)'
    imp: |
        import os
        return os.sep
""",
    "inline-syntax.yaml": "plugins: [inline]\nitem_fields:\n    bad: 'def ('\n    ok: track + 1\n",
    # 5.18a: rewrite, advancedrewrite, substitute, bucket
    "rewrite.yaml": """\
plugins: [rewrite]
rewrite:
    artist .*artist a.*: Rewritten A
    artist the zebras: Zebras, The
    genres rock: Classic Rock
    albumartist zoo.*: Zoo Rewritten
    title hit: HIT
""",
    "advancedrewrite.yaml": """\
plugins: [advancedrewrite]
advancedrewrite:
    - artist artist b: B (simple)
    - match: "album:'Album One' track:1"
      replacements:
          title: First of Album One
          artists: [Uno, Dos]
    - match: year:..2000
      replacements:
          genres: Old
          album: Before 2000
""",
    "substitute.yaml": """\
plugins: [substitute]
substitute:
    '^(.*?)(,| &| and| feat).*': '\\1'
    '^the (.*)': '\\1, The'
    '[aeiou]': '_'
paths:
    default: '%substitute{{$albumartist}}/$album/$title'
""",
    "bucket.yaml": """\
plugins: [bucket]
bucket:
    bucket_year: ['1990s', '2000-09', '2010']
    bucket_alpha: ['A-F', 'G-M', 'N-S', 'TUVWXYZ', 'numbers']
    bucket_alpha_regex:
        numbers: '^[0-9]'
""",
    "rewrite-bad.yaml": "plugins: [rewrite]\nrewrite:\n    artist: Nobody\n",
    "advancedrewrite-bad.yaml": ("plugins: [advancedrewrite]\nadvancedrewrite:\n"
                                 "    - match: 'artist:A'\n      replacements: {{}}\n"),
    "advancedrewrite-list.yaml": ("plugins: [advancedrewrite]\nadvancedrewrite:\n"
                                  "    - match: 'artist:A'\n      replacements:\n"
                                  "          artist: [C, D]\n"),
    "bucket-bad.yaml": "plugins: [bucket]\nbucket:\n    bucket_alpha: ['$%']\n",
    "bucket-extra.yaml": """\
plugins: [bucket]
bucket:
    bucket_year: ['1962-81', '2002', '2012-14']
    extrapolate: yes
""",
    "the-paths.yaml": "plugins: [the]\npaths:\n    default: '%the{{$albumartist}}/%the{{$album}}/$title'\n"
                      "    singleton: '%the{{$artist}}/$title'\n",
}

# ---------------------------------------------------------------------------
# Corpus: (path under src/, format, ffmpeg -metadata tags)


def _album(d, fmt, common, tracks):
    out = []
    for i, t in enumerate(tracks, 1):
        tags = dict(common)
        tags.update(t)
        name = t.get("_file", f"{i:02d}.{fmt}")
        tags.pop("_file", None)
        out.append((f"{d}/{name}", fmt, tags))
    return out


LONG = "Supercalifragilisticexpialidocious " * 7

CORPUS = (
    _album("a01", "mp3", {"artist": "Artist A", "album_artist": "Artist A",
                          "album": "Album One", "date": "2001", "genre": "Rock"},
           [{"title": "First Song", "track": "1/3"},
            {"title": "Second Song", "track": "2/3"},
            {"title": "Third Song", "track": "3/3"}])
    + _album("a02", "mp3", {"artist": "Artist B", "album_artist": "Artist B",
                            "album": "Album One", "date": "2005"},
             [{"title": "Other One", "track": "1"},
              {"title": "Other Two", "track": "2"}])
    + _album("a03", "mp3", {"artist": "Björk Ø", "album_artist": "Björk Ø",
                            "album": "Ünïcödé Album", "date": "1999-05-17"},
             [{"title": "Café ½", "track": "1/5"},
              {"title": "AC/DC Tribute", "track": "2/5"},
              {"title": "über alles", "track": "3/5"},
              {"title": "Ελληνικά", "track": "4/5"},
              {"title": "日本語のタイトル", "track": "5/5"}])
    + _album("a04/Double Album/CD1", "mp3",
             {"artist": "Artist B", "album_artist": "Artist B",
              "album": "Double Album", "date": "2010-03", "disc": "1/2"},
             [{"title": "Disc One Track One", "track": "1/2"},
              {"title": "Disc One Track Two", "track": "2/2"}])
    + _album("a04/Double Album/CD2", "mp3",
             {"artist": "Artist B", "album_artist": "Artist B",
              "album": "Double Album", "date": "2010-03", "disc": "2/2"},
             [{"title": "Disc Two Track One", "track": "1/2"},
              {"title": "Disc Two Track Two", "track": "2/2"}])
    # No album artist: taken as-is, beets calls it a VA album (comp, va_name).
    # (No compilation tag: ffmpeg writes it to MP3s as TXXX:TCMP.)
    + _album("a05", "mp3", {"album": "Various Hits", "date": "1985"},
             [{"title": "Hit One", "artist": "Artist C", "track": "1"},
              {"title": "hit two", "artist": "artist c", "track": "2"},
              {"title": "Hit Three", "artist": "The Zebras", "track": "3"},
              {"title": "Hit Four", "artist": "Zed", "track": "4"}])
    + _album("a06", "mp3", {"artist": "Long Artist", "album_artist": "Long Artist",
                            "album": "Long " + LONG, "date": "2012"},
             [{"title": "Title " + LONG + LONG, "track": "1"}])
    + _album("a07", "mp3", {"artist": "...And Dots", "album_artist": "...And Dots",
                            "album": ".Dot Album", "date": "2007"},
             [{"title": ".dotfile", "track": "1"},
              {"title": "trailing dot.", "track": "2"},
              {"title": "  spaced out  ", "track": "3"}])
    + _album("a08", "mp3", {"album_artist": "Album Artist Only", "album": "No Year"},
             [{"title": "No Artist Here", "track": "1"},
              {"title": "No Track Number"},
              {"title": "", "track": "3"}])
    + _album("a09", "flac", {"ARTIST": "FLAC Artist", "ALBUMARTIST": "FLAC Artist",
                             "ALBUM": "Lossless", "DATE": "2015-11-30"},
             [{"TITLE": "Flac One", "TRACKNUMBER": "1", "DISCNUMBER": "1"},
              {"TITLE": "Flac Two", "TRACKNUMBER": "2", "DISCNUMBER": "1"}])
    + _album("a10", "m4a", {"artist": "Artist A", "album_artist": "Artist A",
                            "album": "AAC Album", "date": "2003"},
             [{"title": "Aac Song", "track": "1/1"}])
    + _album("a11", "mp3", {"artist": "Artist/Slash", "album_artist": "Artist/Slash",
                            "album": "What? Now: <yes> | *star*", "date": "2020"},
             [{"title": 'Quote "this"', "track": "1"},
              {"title": "back\\slash", "track": "2"},
              {"title": "colon: here", "track": "3"},
              {"title": "100% pure $dollar", "track": "4"},
              {"title": "a,comma {brace}", "track": "5"}])
    + _album("a12", "mp3", {"artist": "Artist A", "album_artist": "Artist A",
                            "album": "Album One", "date": "2011"},
             [{"title": "Reissue Track", "track": "1"}])
    + _album("a13", "ogg", {"artist": "Ogg Artist", "album_artist": "Ogg Artist",
                            "album": "Vorbis Album", "date": "1970-01-01"},
             [{"title": "Ogg Song", "tracknumber": "7"}])
    # One track per format and codec, odd lengths: exact length and bitrate.
    + [(f"a14/{n:02d}.{ext}", fmt,
        {"artist": "Format Artist", "album_artist": "Format Artist",
         "album": "Every Format", "date": "2024", "title": f"Fmt {fmt}",
         "track": str(n), "_d": d})
       for n, (ext, fmt, d) in enumerate([
           ("mp3", "mp3", "1.37"), ("mp3", "mp3vbr", "2.71"), ("mp3", "mp3noxing", "1.9"),
           ("flac", "flac", "1.37"), ("m4a", "m4a", "1.37"), ("m4a", "alac", "0.5"),
           ("ogg", "ogg", "3.33"), ("opus", "opus", "1.37"), ("wv", "wv", "2.05"),
           ("aiff", "aiff", "1.37"), ("mp3", "mp3abr", "1.6")], 1)]
    # Tags for MediaFile's other fields, as ffmpeg writes them per format
    # (MP3 frames and TXXX, Vorbis comments, APEv2): alternative keys
    # ("ALBUM ARTIST" before "ALBUMARTIST"), "; " in a genre (a list in
    # beets), ReplayGain/R128, an MP3 "compilation" (ffmpeg's TXXX:TCMP,
    # which beets ignores).
    + [(f"a15/{n:02d}.{ext}", ext,
        {"artist": "Zoo Keeper", "album": "Tag Zoo", "title": f"Zoo {ext}",
         "track": str(n), "date": "2018-06-01", "composer": "Comp One",
         "genre": "Rock; Pop", "ARTISTS": "Zoo Keeper", "grouping": "Grp",
         "MusicBrainz Artist Id": "mbid-1", "REPLAYGAIN_TRACK_GAIN": "-4.20 dB",
         "R128_TRACK_GAIN": "-512", "compilation": "1", "initialkey": "Ebm",
         "BPM": "99", "ISRC": "US1234", "LABEL": "Lbl", "publisher": "Pub",
         "ALBUM ARTIST": "Zoo Band", "album_artist": "Zoo Band 2", "disc": "2/3",
         "lyrics": "la la", "MUSICBRAINZ_ARTISTID": "mbid-v", "ARRANGER": "Arr",
         "ORIGINALDATE": "1999-02", "RELEASETYPE": "album", "SCRIPT": "Latn"})
       for n, ext in enumerate(["mp3", "flac", "ogg", "opus", "wv"], 1)]
    + [
        ("s03/pcm.wav", "wav", {"_d": "1.37"}),
        ("s01/lone1.mp3", "mp3", {"artist": "Solo Artist", "title": "Lone Track",
                                  "date": "2020"}),
        ("s01/lone2.mp3", "mp3", {"artist": "Solo Artist", "title": "Lone Track",
                                  "date": "2021"}),
        ("s01/star.mp3", "mp3", {"artist": "Solo Artist",
                                 "title": "What? Now: *star*", "date": "2020-07"}),
        ("s01/untagged.mp3", "mp3", {}),
        # ID3 genres as numbers and ID3v1 references (mutagen's TCON.genres).
        ("s05/g1.mp3", "mp3", {"title": "Genre Num", "artist": "G", "genre": "80"}),
        ("s05/g2.mp3", "mp3", {"title": "Genre Ref", "artist": "G", "genre": "(17)(RX)Live"}),
        ("s05/g3.mp3", "mp3", {"title": "Genre RX", "artist": "G", "genre": "RX"}),
        ("s05/g4.mp3", "mp3", {"title": "Genre Mixed", "artist": "G", "genre": "Rock; 255"}),
        ("s02/stray.mp3", "mp3", {"artist": "Artist A", "title": "Stray Single",
                                  "album": "Album One", "date": "2001"}),
    ]
)

# MP3s from the `lame` tool, when it's installed (real LAME headers: version,
# VBR method, preset): (path under src/, lame options, tags).
LAME_CORPUS = [
    (f"s04/lame{n:02d}.mp3", opts.split(), {"title": f"Lame {opts}", "artist": "Lame"})
    for n, opts in enumerate(["-b 128", "-V 2", "-V 9", "--abr 160", "--preset insane",
                              "--preset 128", "-V 4 --vbr-old", "-t -b 96"], 1)
]

# Import steps: args to `beet`; the last one is a directory under src/.
# Half run with `threaded: no`, so the library the cases list was built by
# both import pipelines.
_UNTHREADED = ["-c", f"{ROOT}/unthreaded.yaml"]
IMPORTS = [
    ["import", "-A", "-q", d]
    for d in ["a01", "a02", "a03", "a04", "a05", "a06", "a07", "a08"]
] + [
    [*_UNTHREADED, "import", "-A", "-q", d]
    for d in ["a09", "a10", "a11", "a12", "a13", "a14", "a15"]
] + [
    ["import", "-A", "-q", "-s", "s01"],
    ["import", "-A", "-q", "-s", "s02"],
    ["import", "-A", "-q", "-s", "s03"],
    [*_UNTHREADED, "import", "-A", "-q", "-s", "s04"],
    [*_UNTHREADED, "import", "-A", "-q", "-s", "s05"],
]

MODIFIES = [
    ["modify", "-y", "artist:Artist A", "rating=5"],
    ["modify", "-y", "-a", "-I", "album:Double Album", "albummood=epic"],
    ["modify", "-y", "title:café", "comments=hi: there"],
    ["modify", "-y", "artist:Björk Ø", "bpm=120"],
    ["modify", "-y", "title:über", "year=1998"],
    ["modify", "-y", "singleton:true", "grouping=$artist-$year"],
    ["modify", "-y", "title:Second Song", "Flex_Attr=flex"],
    ["modify", "-y", "title:Lone Track", "year:2021", "trackdisambig=live"],
    ["modify", "-y", "-a", "album:AAC Album", "albumtype=live"],
    ["modify", "-y", "artist:The Zebras", "rating=3"],
    ["modify", "-y", "artist:=artist c", "rating=10"],
    ["modify", "-y", "title:First Song", "releasedate=2001-02-03"],
    ["modify", "-y", "title:Hit Four", "comp=0"],
    ["modify", "-y", "-a", "album:Lossless", "catalognum=ABC 123", "label=Indie"],
    ["modify", "-y", "title:Other Two", "title=Other Two (Live)", "year=2006"],
    ["modify", "-y", "album:No Year", "mood!"],
    ["modify", "-y", "title:Ogg", "mood=calm", "rating=7"],
    # Multi-valued fields: assignment ("; " separated), += and -=, the
    # legacy single-valued names, nullable floats and musical keys.
    ["modify", "-y", "title:Hit One", "genres=Pop; Rock", "artists=Artist C; Guest",
     "composers=X; Y", "rg_track_gain=-6.5", "initial_key=Ebm", "mb_artistids=m1; m2"],
    ["modify", "-y", "album:Tag Zoo", "genres+=Jazz; Pop"],
    ["modify", "-y", "album:Tag Zoo", "format:FLAC", "genres-=Rock"],
    ["modify", "-y", "title:Hit Three", "genre=Ska", "composer=Solo"],
    ["modify", "-y", "-a", "album:Vorbis Album", "genres=Ambient; Drone",
     "albumtypes=album; live", "rg_album_gain=1.5"],
]
