"""Corpus, config and cases for diffbeets.py.

Case ids are "<kind>:<args>"; EXPECTED maps a case id to the reason the two
tools are allowed to differ there.
"""

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

# ---------------------------------------------------------------------------
# Queries (each run with `ls`, `ls -a`)

QUERIES = [
    # test_query.py TestGet, translated
    "",
    "one",
    "oNE",
    ":one",
    ":oNE",
    ":sec :ond",
    ":Second",
    "=First Song",
    "=~first song",
    "=~FIRST",
    ":g$",
    "album",
    "sec ond",
    "Second Song",
    "albummood:epic",
    "artist::A.+t",
    "artist:artIST",
    "ArTiST:zebras",
    "comments:hi",
    "comments:hi: there",
    "comments:hi\\: there",
    "comp:true",
    "comp:false",
    "comp:1",
    "comp:no",
    "flex_attr:flex",
    "Flex_Attr:flex",
    "popebear",
    "pope:bear",
    "singleton:true",
    "singleton:1",
    "singleton:false",
    "singleton:0",
    "singleton:yes",
    "title:ond",
    "title::^sec",
    "title::Sec",
    "year:2001",
    "year:2000..2002",
    "year:..2000",
    "year:2010..",
    "year:2005..2001",
    "year:0",
    "year:199a",
    "year::199(",
    "title::(",
    "xyzzy:nonsense",
    "rating:5",
    "rating:3..7",
    "rating:..5",
    "rating:10",
    "bpm:120",
    "bpm:110..125",
    "bpm:12",
    "bpm:0",
    "track:1",
    "track:..1",
    "disc:2",
    "length:..0:02",
    "length:0:00..0:02",
    "length:..1",
    "bitrate:128000..",
    "bitrate:..100000",
    "samplerate:48000",
    "format:FLAC",
    "format:mp3",
    "format:=MP3",
    "albumtype:live",
    "catalognum:ABC",
    "catalognum:ABC Lossless",
    "label:indie",
    "mood:calm",
    "mood:",
    "title:",
    "artist:",
    "artist:=",
    "title:=",
    "id:1",
    "id:3..5",
    "album_id:2",
    "albumartist:various",
    "trackdisambig:live",
    "grouping:Solo",
    "releasedate:2001",
    # negation (TestGet.test_negation_prefix)
    "-artist::A.+t",
    "-:g$",
    "sec -bar",
    "sec -title:bar",
    "-ond",
    "^ond",
    "^title:sec",
    "-title:sec",
    "-singleton:true",
    "-comp:true",
    "-year:2000..2010",
    "-rating:5",
    "^rating:5",
    "-",
    "^",
    # OR (test_queryparse.py ParseSortedQueryTest)
    "first , second",
    "first, second",
    "first,second",
    "first , second ,",
    ", first , second",
    "first ,",
    "artist:zed , year:2001",
    "-artist:zed , year:2001",
    "Hit , Song title:one",
    # escapes and colons
    "test\\:val",
    ":test\\:regexp",
    "colon\\: here",
    "title:colon: here",
    "What? Now",
    "*star*",
    "100%",
    "$dollar",
    "a,comma",
    "{brace}",
    "Quote \"this\"",
    "back\\slash",
    "back\\\\slash",
    "title::back\\\\slash",
    "_",
    "%",
    # unicode
    "café",
    "CAFÉ",
    "cafe",
    "Café ½",
    "½",
    "über",
    "ÜBER",
    "Björk",
    "björk ø",
    "artist:=Björk Ø",
    "artist:=~björk ø",
    "ελληνικά",
    "Ελληνικά",
    "日本語",
    ":^日本",
    "title::über",
    "title::(?i)ÜBER",
    "title::Ü",
    # album-level fields from item queries (TestRelatedQueries)
    "albumartist:Artist B",
    "album:Double disc:2",
    "catalognum:ABC Flac",
    "albummood:epic title:Two",
    "label:Indie",
    "title:Track Two",
    "albumtotal:4",
    # dates (test_datequery.py)
    "added:2000..",
    "added:..2001",
    "added:2000..2001",
    "added:-1d..",
    "added:-1d..1d",
    "added:-2d..-1d",
    "added:-4w..",
    "added:-4m..+4m",
    "added:-4y..",
    "added:4d..",
    "added:..+4d",
    "added:..-4d",
    "added:+4d..-4d",
    "added:..",
    "added:2024..2020",
    "added:1409830085..1412422089",
    "added:12-34-56-78",
    "added:2001-01-0a",
    "added:200a",
    "added:..2aa",
    "added:2000|2001",
    "added:5|3d",
    "added:2000-01-01T12",
    "added:2000-01-01t12",
    "added:2000-01-01 12",
    "added:2000-01-01x12",
    "added:2000-01-01T12:30..2000-01-01T12:31",
    "added:2000-01-01T12:30:50..2000-01-01T12:30:55",
    "added:1999-12..2000-02",
    "added:2001..2000-06",
    "mtime:2000..",
    "mtime:..2000",
    # sorts (test_sort.py)
    "year+",
    "year-",
    "flex1-",
    "rating+",
    "rating-",
    "rating- title+",
    "albummood+ title+",
    "path+",
    "path-",
    "year+ path+",
    "artpath+",
    "artist+",
    "artist-",
    "artist+ title-",
    "albumartist-",
    "album+ disc- track-",
    "title+",
    "title-",
    "length+",
    "bitrate- title+",
    "format+ title+",
    "added- title+",
    "foo+",
    "foo-",
    "--",
    "-+",
    "+-",
    "++",
    "-foo-",
    "-foo+",
    "---",
    "foo+ id+",
    "id-",
    "-bar+",
    "track+",
    "track-",
    "comp+ title+",
    "singleton+ title+",
    "year+ Song",
    "artist:artist year- title+",
    # path queries (relative to the library directory, implicit)
    "path:Compilations",
    "path:compilations",
    "path:Non-Album",
    "path:Non-Album/Solo Artist",
    "path::Lone",
    "path::\\.flac$",
    "path:Artist A/Album One",
    "path:lib/Artist A",
    "path:xyzzy",
    "path:Artist",
    # implicit path queries (cases run in the library directory, so these
    # exist; test_query.py TestPathQuery.test_implicit)
    "Lossless/",
    "Lossless/FLAC",
    "Lossless/ , Compilations/",
    "Lossless/ Flac",
    "nope/",
    "Lossless/:x",
    "title:/a/b",
    "../lib/Lossless/",
    "./Compilations",
    "path:./Compilations",
    "path:Lossless/../Compilations",
    # multi-valued (beets 2.14 lists): SQL matches the joined string
    "genres:rock",
    "genre:Rock",
    "Genre:Rock",
    "genres:pop",
    "genres::^Pop",
    "genres::Rock$",
    "genres:=Pop",
    "genres:=Ska",
    "genres:=Pop; Rock",
    "genres:=~ska",
    "genres:Rock genres:Pop",
    "genres:Jazz -format:FLAC",
    "genres:",
    "genre:jazz , composer:solo",
    "composer:X",
    "composers:Y",
    "composers:Comp",
    "artists:Guest",
    "artists::^Artist C",
    "albumtypes:live",
    "ska",
    "drone",
    # new fields read from tags; nullable floats
    "rg_track_gain:-7..-6",
    "rg_track_gain:..0",
    "rg_track_gain:-4.2",
    "rg_album_gain:1.5",
    "rg_album_gain:",
    "r128_track_gain:-2",
    "initial_key:D#m",
    "initial_key:Ebm",
    "initial_key:",
    "label:Lbl",
    "label:Pub",
    "bpm:99",
    "isrc:US1234",
    "mb_artistid:mbid",
    "mb_artistids:m2",
    "encoder:Lav",
    "comp:true album:Tag",
    "arrangers:Arr",
    "original_year:1999",
    "script:Latn",
    "albumtype:album",
    "genres:Folk",
    "genres:Remix artist:G",
]

# ---------------------------------------------------------------------------
# Templates (each run with `ls -f`, `ls -a -f`)

TEMPLATES = [
    # test_functemplate.py
    "",
    "hello world",
    "$$",
    "a $$ b",
    "$$ hello",
    "hello $$",
    "a $% b",
    "a $, b",
    "a $} b",
    "a $ b",
    "a % b",
    "a { b",
    "a } b",
    "a , b",
    "$title",
    "hello $title world",
    "hello${title}world",
    "a ${ b",
    "a ${} b",
    "foo %bar",
    "foo %bar baz",
    "foo %bar{ baz",
    "foo %bar{bar,bar baz",
    "%foo{}",
    "%lower{bar$,baz}",
    "%lower{bar$}baz}",
    "%lower{$title,baz}",
    "%lower{%upper{x},baz}",
    "%lower{%upper{baz}}",
    "hello, %lower{BAR}",
    "hello,$title,$artist",
    "$bar",
    "%lower{FOO}",
    "A %lower{FOO} B",
    "%lower{%lower{FOO}}",
    "%lower{$title}",
    "%lower{a,b,c,d,e}",
    "%bar{}",
    "%lower",
    "${title",
    "$title}rest",
    "}after",
    "${artist}x${album}",
    "${not a field}",
    "$title$artist",
    "$1",
    "$_x",
    "$tit-le",
    # test_library.py TestDestinationFunction
    "%upper{foo}",
    "%upper{$title}",
    "%capitalize{$title}",
    "%capitalize{}",
    "%title{$title}",
    "%title{it's a test o'clock}",
    "%asciify{$title}",
    "%asciify{$artist $album}",
    "%left{$title, 4}",
    "%left{$title,4}",
    "%left{$title,-3}",
    "%right{$title,3}",
    "%right{$title,-3}",
    "%right{$title,x}",
    "%left{$title}",
    "%if{,foo}",
    "%if{false,foo}",
    "%if{False,foo}",
    "%if{0,foo}",
    "%if{00,foo}",
    "%if{ 1 ,foo}",
    "%if{-1,foo}",
    "%if{bar,foo}",
    "%if{,foo,baz}",
    "%if{false,foo,baz}",
    "%if{$comp,comp,notcomp}",
    "%if{$bpm,bpm $bpm,nobpm}",
    "%if{$track,$track,none}",
    "%if{$rating,rated $rating}",
    "%if{}",
    "%if{a,b,c,d}",
    "%foo{bar}",
    "%ifdef{rating}",
    "%ifdef{bar}",
    "%ifdef{bar,baz}",
    "%ifdef{rating,yes,no}",
    "%ifdef{albummood,yes,no}",
    "%ifdef{title,yes,no}",
    "%ifdef{mood,yes,no}",
    "%first{Alice / Bob / Eve}",
    "%first{Alice / Bob / Eve,2}",
    "%first{Alice / Bob / Eve,2,1}",
    "%first{Alice; Bob; Eve,2,0,; ,-}",
    "%first{$artist,1,0, ,x}",
    "%first{}",
    "%time{$added,%Y}",
    "%time{2001-02-03,%Y}",
    "%time{$releasedate,%d}",
    "%time{$added}",
    "%time{nonsense,%Y}",
    "%aunique{}",
    "%aunique{albumartist album,year}",
    "%aunique{albumartist album,year,()}",
    "%aunique{albumartist album,year,}",
    "%aunique{albumartist album,albumtype year label}",
    "%aunique{album,albumartist}",
    "%aunique{album,albumartist,[]}",
    "%aunique{album,xyzzy}",
    "%sunique{}",
    "%sunique{artist title,year trackdisambig}",
    "%sunique{artist title,year,{}}",
    "%tunique{}",
    "%tunique{title,track}",
    # fields and formatting (test_library.py TestDestination, ItemFormattedMapping)
    "$artist - $album - $title",
    "$albumartist - $album",
    "$track $disc $year $month $day",
    "$tracktotal $disctotal",
    "$bitrate $samplerate $format $length",
    "$bpm $comp $singleton",
    "$rating $Flex_Attr $flex_attr",
    "$albummood",
    "$albumtype",
    "$comments",
    "$grouping",
    "$releasedate",
    "$catalognum $label",
    "$album_id",
    "$albumtotal",
    "$has_cover_art",
    "$original_year $original_month",
    "$channels $bitdepth",
    "$encoder",
    "$mood",
    "$path",
    "$artpath",
    "$filesize",
    "%if{$singleton,single,album}",
    "%if{$albumtype,$albumtype}",
    "%upper{%left{$artist,1}}/$artist",
    "$title (%if{$year,$year,unknown})",
    "%right{000$track,3}",
    "${title}${year}",
    # multi-valued fields, legacy names, MediaFile fields
    "$genres",
    "$genre $composer",
    "$artists | $composers | $arrangers",
    "%first{$genres}",
    "%first{$genres,1,1}",
    "$mb_artistids $mb_artistid",
    "$albumtypes $albumtype",
    "$rg_track_gain $rg_track_peak $rg_album_gain $rg_album_peak",
    "$r128_track_gain $r128_album_gain",
    "$initial_key",
    "$label $isrc $bpm $lyrics $script",
    "$disc/$disctotal $original_year-$original_month",
    "$albumartist $albumartists $grouping $comp",
    "$lyricists $remixers $subtitle $work $trackdisambig",
    "%if{$genres,$genres,none}",
    "$title|$bitrate_mode|$encoder_info|$encoder_settings",
]

# Path formats for `mv -p -d <root>/dest`, given with `-c` so they replace
# `paths:` (each as `default`, plus the shared singleton/comp formats).
PATHS = [
    "$albumartist/$album%aunique{}/$track $title",
    "$artist/$album/$disc-$track $title",
    "$artist/$title",
    "$title",
    "%upper{$artist}/%lower{$album}/$title",
    "$genre/$artist/$title",
    "$year/$month/$day/$title",
    "$albumartist/$album ($year)/$track - $title - $bitrate $samplerate $format",
    "$artist/$album/%if{$comp,comp/}$title",
    "/$artist/$title",
    "$artist//$title",
    "$artist/$album/",
    "$xyzzy/$title",
    "%asciify{$artist}/$title",
    "$artist/%first{$title,1,0,/}",
    "$artist/$title%sunique{}",
    "$albumartist/$album%aunique{albumartist album,year albumtype}/$title",
    "$albumartist/$album/$title%tunique{}",
    "$albumartist/$album%aunique{album,albumartist label}/$track",
    "$rating/$title",
    "$albummood/$title",
    "${artist}_${title}",
    "$comments/$title",
    "$grouping/$title",
    "$track/$title",
    "$artist - $album/$title",
    "$title.$format",
    "%time{$added,%Y}/$title",
    "$length/$title",
    "$genres/$title",
    "%first{$genres}/$albumartist/$title",
]


def _paths_config(fmt):
    def q(s):
        return "'" + s.replace("'", "''") + "'"
    return (
        "paths:\n"
        f"    default: {q(fmt)}\n"
        f"    singleton: {q('Singles/' + fmt)}\n"
        f"    comp: {q('Comps/' + fmt)}\n"
    )


for _i, _fmt in enumerate(PATHS):
    EXTRA_CONFIGS[f"paths{_i:02d}.yaml"] = _paths_config(_fmt).replace("{", "{{").replace("}", "}}")

# The shared config's paths, with other options layered on top.
MV_OPTION_CONFIGS = ["replace.yaml", "asciify.yaml", "sep.yaml", "maxlen.yaml", "empty.yaml",
                     "art.yaml"]

# Sort configs, with `ls`
SORT_CONFIG_QUERIES = ["", "artist+", "album+", "flex1+", "rating+", "title+", "Hit"]


# Queries also run without `--`, to compare the option-parsing error.
RAW_DASH = ["-ond", "-+", "---", "-bar+"]


def _query_args(q):
    """A query string as `beet` arguments (split on spaces, as a shell
    would); `--` first when a term starts with `-`, so it isn't an option."""
    args = q.split(" ") if q else []
    if any(a.startswith("-") for a in args):
        args = ["--", *args]
    return args


DEST = f"{ROOT}/dest"

# Exact audio properties (mutagen's length and bitrate): point and
# boundary queries, the formatted fields, and stats per format.
PROPERTY_QUERIES = [
    "length:1.37", "length:1", "length:..1.0449", "length:1.0449..1.05",
    "length:1.37..1.371", "length:1.3..1.4", "length:1.5..", "length:3.3..3.35",
    "bitrate:705600", "bitrate:2304000", "bitrate:..64000", "bitrate:64000..100000",
    "bitrate:128000", "bitrate:127000..128999", "bitrate:96000..98999",
    "bitrate:32000", "bitrate:100000..", "length:..0.6", "length:0.5",
]
PROPERTY_FORMATS = ["$title $length $bitrate $format $samplerate $bitdepth $channels",
                    "${length}|${bitrate}",
                    "$title|$bitrate_mode|$encoder_info|$encoder_settings"]
STATS_FORMATS = ["MP3", "FLAC", "AAC", "ALAC", "OGG", "Opus", "WavPack", "AIFF", "WAVE"]

# `stats` options and queries (beets' show_stats; -l is beets' limit option).
STATS = [[], ["-e"], ["Hit"], ["-e", "Hit"], ["-l", "3"], ["-e", "-l", "3"],
         ["--limit", "1", "album:Lossless"], ["-l", "0"], ["-l", "100"],
         ["-l", "2", "title+"], ["-e", "-l", "4", "singleton:true"]]


# Tag round trips (after the cases above): each tool writes its library's
# tags into its files (`write`, then `write -f` as ID3v2.3), the files
# are compared as MediaFile reads them; then both tools import the files
# reference beets wrote and list every tag field.
TAG_WRITES = [("write", ["write"]), ("write id3v23", ["-c", f"{ROOT}/id3v23.yaml", "write", "-f"])]
# scrub: every tag removed (FLAC pictures, AIFF and WAV kept, as Mutagen), then
# the library's tags written back.
TAG_WRITES += [("scrub -W", ["-p", "scrub", "scrub", "-W"]), ("scrub", ["-p", "scrub", "scrub"])]
TAG_FIELDS = [
    "acoustid_fingerprint", "acoustid_id", "album", "albumartist", "albumartist_credit",
    "albumartist_sort", "albumartists", "albumartists_credit", "albumartists_sort",
    "albumdisambig", "albumstatus", "albumtype", "albumtypes", "arrangers", "artist",
    "artist_credit", "artist_sort", "artists", "artists_credit", "artists_sort", "asin",
    "barcode", "bpm", "catalognum", "comments", "comp", "composer_sort", "composers",
    "country", "day", "disc", "disctitle", "disctotal", "encoder", "genres", "grouping",
    "initial_key", "isrc", "label", "language", "lyricists", "lyrics", "mb_albumartistid",
    "mb_albumartistids", "mb_albumid", "mb_artistid", "mb_artistids", "mb_releasegroupid",
    "mb_releasetrackid", "mb_trackid", "mb_workid", "media", "month", "original_day",
    "original_month", "original_year", "r128_album_gain", "r128_track_gain", "remixers",
    "rg_album_gain", "rg_album_peak", "rg_track_gain", "rg_track_peak", "script",
    "subtitle", "title", "track", "tracktotal", "year",
]
# (No $path: which of two same-titled singletons gets the plain name depends
# on the import order.)
TAG_TEMPLATE = "|".join(f"{f}=${f}" for f in TAG_FIELDS)


# the plugin's %the{} (with -c the*.yaml); without the plugin it stays as written.
THE_CONFIGS = ["the.yaml", "the-custom.yaml", "the-strip.yaml", "the-none.yaml"]
THE_TEMPLATES = ["%the{$artist}|%the{$title}|%the{$album}", "%the{The A}|%the{an  apple}|%the{A}",
                 "%the{}", "%the{a,b}", "%the{%the{The The X}}"]


def cases():
    out = []
    for q in RAW_DASH:
        out.append((f"ls (no --):{q}", ["ls", q]))
    for q in QUERIES:
        args = _query_args(q)
        out.append((f"ls:{q}", ["ls", *args]))
        out.append((f"ls -a:{q}", ["ls", "-a", *args]))
    for q in ["", "Lone", "Double", "Hit", "café", "singleton:true"]:
        args = q.split(" ") if q else []
        out.append((f"ls -p:{q}", ["ls", "-p", *args]))
        out.append((f"ls -a -p:{q}", ["ls", "-a", "-p", *args]))
    for t in TEMPLATES:
        out.append((f"ls -f:{t}", ["ls", "-f", t]))
        out.append((f"ls -a -f:{t}", ["ls", "-a", "-f", t]))
    for i, fmt in enumerate(PATHS):
        out.append((f"mv -p paths:{fmt}",
                    ["-c", f"{ROOT}/paths{i:02d}.yaml", "mv", "-p", "-d", DEST]))
    for name in MV_OPTION_CONFIGS:
        out.append((f"mv -p -c {name}", ["-c", f"{ROOT}/{name}", "mv", "-p", "-d", DEST]))
        out.append((f"mv -p -a -c {name}", ["-c", f"{ROOT}/{name}", "mv", "-p", "-a", "-d", DEST]))
    for args in STATS:
        out.append((f"stats:{' '.join(args)}", ["stats", *args]))
    for fmt in STATS_FORMATS:
        out.append((f"stats:format:{fmt}", ["stats", f"format:{fmt}"]))
        out.append((f"stats -e:format:{fmt}", ["stats", "-e", f"format:{fmt}"]))
    for q in PROPERTY_QUERIES:
        out.append((f"ls props:{q}", ["ls", "-f", "$title", q]))
    for f in PROPERTY_FORMATS:
        out.append((f"ls -f props:{f}", ["ls", "-f", f, "album:Every Format", ",", "format:WAV"]))
    out.append(("fields", ["fields"]))
    out.append(("mv -p", ["mv", "-p"]))
    out.append(("mv -p -d", ["mv", "-p", "-d", DEST]))
    out.append(("mv -p -a -d", ["mv", "-p", "-a", "-d", DEST]))
    out.append(("mv -p -d Hit", ["mv", "-p", "-d", DEST, "Hit"]))
    for name in THE_CONFIGS:
        for t in THE_TEMPLATES:
            out.append((f"-c {name} ls -f:{t}", ["-c", f"{ROOT}/{name}", "ls", "-f", t]))
    out.append(("ls -f:%the{$artist} (no plugin)", ["ls", "-f", "%the{$artist}"]))
    out.append(("-c the-paths.yaml mv -p", ["-c", f"{ROOT}/the-paths.yaml", "mv", "-p"]))
    for name in ["nocaseins.yaml", "sortcfg.yaml"]:
        for q in SORT_CONFIG_QUERIES:
            args = _query_args(q)
            out.append((f"-c {name} ls:{q}", ["-c", f"{ROOT}/{name}", "ls", *args]))
            out.append((f"-c {name} ls -a:{q}", ["-c", f"{ROOT}/{name}", "ls", "-a", *args]))
    for args in INFO_ARGS:
        out.append((f"info:{' '.join(args)}", ["-p", "info", "info", *args]))
    for args in MISSING_ARGS:
        out.append((f"missing:{' '.join(args)}", [*_NOMB, "-p", "missing", *args]))
    for args in DUP_ARGS:
        out.append((f"dup:{' '.join(args)}", ["-p", "duplicates", *args]))
    for args in TYPES_ARGS:
        out.append((f"types:{' '.join(args)}", args))
    return out


# missing plugin, offline: counts (`$missing`), formats and the album mode
# (no album artist in the corpus has a MusicBrainz id, so no lookups; the
# musicbrainz plugin isn't loaded, so the track mode never looks up either).
_NOMB = ["-c", f"{ROOT}/nomb.yaml"]
EXTRA_CONFIGS["nomb.yaml"] = "musicbrainz:\n    host: 127.0.0.1:9\n"
MISSING_ARGS = [
    ["missing"],
    ["miss", "-c"],
    ["missing", "-t"],
    ["missing", "-c", "-t"],
    ["missing", "-c", "-f", "$albumartist / $album"],
    ["missing", "-c", "album:Every Format"],
    ["missing", "-c", "-l", "2"],
    ["missing", "-t", "Hit"],
    ["missing", "-a"],
    ["missing", "-a", "-t"],
    ["missing", "-a", "--release-types", "ep,single", "-t"],
    ["ls", "-a", "-f", "$album: $missing/$albumtotal"],
    ["ls", "-f", "$title $missing"],
    ["ls", "-a", "-f", "%ifdef{missing,yes,no}"],
    ["fields"],
]


# info plugin: tags read from files (paths or queries), library values (-l,
# -a), summaries, keys and formats. No added/mtime: they differ per tool.
INFO_ARGS = [
    [],
    ["album:Every Format"],
    ["album:Tag Zoo"],
    ["-s", "album:Tag Zoo"],
    ["-s", "album:Every Format"],
    ["-k", "album:Tag Zoo"],
    ["-i", "title,art,genre,genres,date,nope,images", "-i", "year,path", "album:Tag Zoo"],
    ["-f", "$title $length $format $bitrate", "album:Every Format"],
    ["--limit", "2", "album:Every Format"],
    ["Format Artist/Every Format/01 Fmt mp3.mp3", "album:Tag Zoo"],
    ["no-such-thing"],
    ["-l", "-k", "album:Tag Zoo"],
    ["-l", "-i", "title,artist,album,nope,format,length,path", "album:Every Format"],
    ["-l", "-s", "-i", "title,album,albumartist,year,genres"],
    ["-l", "-f", "$track. $title - $artist ($length)", "album:Every Format"],
    ["-a", "-i", "album,albumartist,albumtotal,year,path,nope"],
    ["-a", "-k", "album:Tag Zoo"],
    ["-a", "-s", "-i", "albumartist,year"],
]


# ---------------------------------------------------------------------------
# Deliberate or known differences: case id -> reason

_CLI = ("beets' optparse and our clap word an unknown option differently "
        "(both exit 1); with `--` the query itself is compared")
_REGEX = ("regex syntax errors carry the regex engine's own message (Python re "
          "vs fancy-regex); the `invalid query: '...': '...' is not a regular "
          "expression:` part is the same")
_ALBUM_SORT_DUP = ("beets bug: sorting albums by an item field LEFT JOINs the "
                   "items without grouping, so each album is listed once per "
                   "item; beets-rs lists each album once")
_ALBUM_SORT_CRASH = ("beets bug: sorting albums by a field both tables have "
                     "(after the item join) crashes with sqlite3 "
                     "'ambiguous column name'; beets-rs sorts by the album's field")

_EMPTY_TAGS = ("beets writes an empty list field to APEv2 as an empty value (beets-rs "
               "removes the tag; MediaFile reads both as no value) and reads that back "
               "as [''], so its `write -f` after its first `write` lists the WavPack "
               "file's empty list fields as changed")

EXPECTED = {
    "tags:write id3v23": _EMPTY_TAGS,
    **{f"ls (no --):{q}": _CLI for q in RAW_DASH},
    "ls:year::199(": _REGEX,
    "ls -a:year::199(": _REGEX,
    "ls:title::(": _REGEX,
    "ls -a:title::(": _REGEX,
    **{f"ls -a:{q}": _ALBUM_SORT_DUP for q in [
        "rating- title+", "albummood+ title+", "title+", "title-", "length+",
        "bitrate- title+", "format+ title+", "track+", "track-",
        "singleton+ title+",
    ]},
    "-c nocaseins.yaml ls -a:title+": _ALBUM_SORT_DUP,
    "-c sortcfg.yaml ls -a:title+": _ALBUM_SORT_DUP,
    **{f"ls -a:{q}": _ALBUM_SORT_CRASH for q in [
        "artist+ title-", "album+ disc- track-", "added- title+", "comp+ title+",
        "artist:artist year- title+",
    ]},
}


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
}

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
    ("zero-q-asis", "fx", ["-q", "-C", "--quiet-fallback=asis"],
     "plugins: [musicbrainz, zero]\nzero:\n    fields: title\n", []),
]

# ---------------------------------------------------------------------------
# Art phase (artphase.py): (name, music, {file: image}, config, steps).
# A step is a list of `beet` arguments, or (arguments, stdin).

_IMP = ["import", "-A", "-q", "{music}"]
_LSA = ["ls", "-a", "-f", "$album $artpath $art_source"]
_EMBED = "{{plugins: [fetchart, embedart], import: {{write: yes}}}}"
_EMBEDART = "{{plugins: [embedart]}}"

ART_CASES = [
    # filesystem source on import
    ("fs-copy", "tracks", {"cover.jpg": "sq300.jpg", "back.png": "wide.png"}, "", [_IMP, _LSA]),
    ("fs-move", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{import: {{copy: no, move: yes}}}}", [_IMP, _LSA]),
    ("fs-delete", "tracks", {"cover.jpg": "sq300.jpg"}, "{{import: {{delete: yes}}}}", [_IMP]),
    ("fs-any-image", "tracks", {"scan.png": "wide.png"}, "", [_IMP]),
    ("fs-cautious", "tracks", {"scan.png": "wide.png"}, "{{fetchart: {{cautious: yes}}}}",
     [_IMP, _LSA]),
    ("fs-cover-names", "tracks", {"cover.jpg": "sq300.jpg", "back.png": "wide.png"},
     "{{fetchart: {{cover_names: [back, cover]}}}}", [_IMP]),
    ("fs-name-precedence", "tracks",
     {"front.jpg": "sq300.jpg", "album-cover.png": "wide.png", "folder.jpg": "small.jpg"},
     "", [_IMP]),
    ("fs-fallback", "tracks", {}, "{{fetchart: {{fallback: '{src}/sq300.png'}}}}", [_IMP]),
    ("fs-no-images", "tracks", {}, "", [_IMP, _LSA]),
    ("fs-hidden-ignored", "tracks", {".cover.jpg": "sq300.jpg"}, "", [_IMP]),
    ("store-source", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{fetchart: {{store_source: yes}}}}", [_IMP, _LSA]),
    ("auto-off", "tracks", {"cover.jpg": "sq300.jpg"}, "{{fetchart: {{auto: no}}}}",
     [_IMP, _LSA]),
    ("art-filename", "tracks", {"cover.jpg": "sq300.jpg"}, "{{art_filename: 'folder-$album'}}",
     [_IMP]),
    # image checks and processing
    ("minwidth", "tracks", {"cover.jpg": "sq300.jpg"}, "{{fetchart: {{minwidth: 500}}}}",
     [_IMP, _LSA]),
    ("minwidth-ok", "tracks", {"cover.jpg": "small.jpg", "front.jpg": "sq600.jpg"},
     "{{fetchart: {{minwidth: 500}}}}", [_IMP]),
    ("enforce-ratio", "tracks", {"cover.jpg": "near.jpg", "front.png": "sq300.png"},
     "{{fetchart: {{enforce_ratio: yes}}}}", [_IMP]),
    ("enforce-ratio-px", "tracks", {"cover.jpg": "near.jpg", "front.png": "sq300.png"},
     "{{fetchart: {{enforce_ratio: 10px}}}}", [_IMP]),
    ("enforce-ratio-percent", "tracks", {"cover.jpg": "near.jpg", "front.png": "sq300.png"},
     "{{fetchart: {{enforce_ratio: 1%}}}}", [_IMP]),
    ("enforce-ratio-wide", "tracks", {"cover.png": "wide.png"},
     "{{fetchart: {{enforce_ratio: 50%}}}}", [_IMP]),
    ("maxwidth", "tracks", {"cover.jpg": "sq300.jpg"}, "{{fetchart: {{maxwidth: 100}}}}", [_IMP]),
    ("maxwidth-wide", "tracks", {"cover.png": "wide.png"},
     "{{fetchart: {{maxwidth: 100, quality: 75}}}}", [_IMP]),
    ("maxwidth-larger", "tracks", {"cover.jpg": "small.jpg"},
     "{{fetchart: {{maxwidth: 100}}}}", [_IMP]),
    ("cover-format", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{fetchart: {{cover_format: PNG}}}}", [_IMP, _LSA]),
    ("cover-format-same", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{fetchart: {{cover_format: JPEG}}}}", [_IMP]),
    ("max-filesize", "tracks", {"cover.jpg": "big.jpg"},
     "{{fetchart: {{max_filesize: 20000}}}}", [_IMP]),
    ("deinterlace", "tracks", {"cover.png": "sq300.png"}, "{{fetchart: {{deinterlace: yes}}}}",
     [_IMP]),
    ("sources-invalid", "tracks", {}, "{{fetchart: {{sources: [nosuch]}}}}", [_IMP]),
    ("sources-empty", "tracks", {}, "{{fetchart: {{sources: []}}}}", [["ls"]]),
    # the fetchart command
    ("cmd", "tracks", {"front.jpg": "sq300.jpg"}, "{{fetchart: {{auto: no}}}}",
     [_IMP, ["fetchart"], ["fetchart"], ["fetchart", "-q"], ["fetchart", "-f"], _LSA]),
    ("cmd-in-place", "tracks", {"front.jpg": "sq300.jpg", "back.png": "wide.png"},
     "{{fetchart: {{auto: no, store_source: yes}}, import: {{copy: no}}}}",
     [_IMP, ["fetchart"], ["fetchart"], ["fetchart", "-q"], ["fetchart", "-f"], _LSA]),
    ("cmd-in-place-maxwidth", "tracks", {"scan.png": "wide.png"},
     "{{fetchart: {{auto: no, maxwidth: 100, cover_format: JPEG}}, import: {{copy: no}}}}",
     [_IMP, ["fetchart"], _LSA]),
    ("cmd-none", "tracks", {}, "{{fetchart: {{auto: no}}}}",
     [_IMP, ["fetchart"], ["fetchart", "-l", "0"], ["fetchart", "nomatch"]]),
    ("cmd-not-enabled", "tracks", {}, "{{plugins: []}}", [["fetchart"]]),
    # cover_art_url: downloads from the local server
    ("url", "tracks", {},
     "{{fetchart: {{sources: [cover_art_url], store_source: yes}}}}",
     [_IMP, ["modify", "-a", "-y", "cover_art_url={url}/img/sq300.png"], ["fetchart"], _LSA]),
    ("url-item-field", "tracks", {}, "{{fetchart: {{sources: [cover_art_url]}}}}",
     [_IMP, ["modify", "-y", "title:One", "cover_art_url={url}/img/sq300.jpg"], ["fetchart"],
      _LSA]),
    ("url-mislabeled", "tracks", {}, "{{fetchart: {{sources: [cover_art_url]}}}}",
     [_IMP, ["modify", "-a", "-y", "cover_art_url={url}/mislabeled.jpg"], ["fetchart"]]),
    ("url-missing", "tracks", {}, "{{fetchart: {{sources: [cover_art_url]}}}}",
     [_IMP, ["modify", "-a", "-y", "cover_art_url={url}/img/nope.jpg"], ["fetchart"]]),
    ("url-not-image", "tracks", {}, "{{fetchart: {{sources: [cover_art_url]}}}}",
     [_IMP, ["modify", "-a", "-y", "cover_art_url={url}/page"], ["fetchart"]]),
    ("url-maxwidth", "tracks", {}, "{{fetchart: {{sources: [cover_art_url], maxwidth: 64}}}}",
     [_IMP, ["modify", "-a", "-y", "cover_art_url={url}/img/big.jpg"], ["fetchart"]]),
    ("url-on-import-asis", "tracks", {}, "{{fetchart: {{sources: [cover_art_url]}}}}",
     [["import", "-A", "-q", "--set", "cover_art_url={url}/img/sq300.jpg", "{music}"], _LSA]),
    ("url-fetch-for-asis", "tracks", {},
     "{{fetchart: {{sources: [cover_art_url], fetch_for_asis: yes}}}}",
     [["import", "-A", "-q", "--set", "cover_art_url={url}/img/sq300.jpg", "{music}"], _LSA]),
    ("url-after-filesystem", "tracks", {"scan.jpg": "small.jpg"},
     "{{fetchart: {{sources: [filesystem, cover_art_url], cautious: yes, "
     "fetch_for_asis: yes}}}}",
     [["import", "-A", "-q", "--set", "cover_art_url={url}/img/sq300.png", "{music}"], _LSA]),
    # the import hooks: fetch_art is an import stage (albums only), assign_art
    # and clearart_on_import listen to import_task_files, in plugin order
    ("stage-singleton", "tracks", {"cover.jpg": "sq300.jpg"}, "",
     [["import", "-A", "-q", "-s", "{music}"], ["ls"], _LSA]),
    ("stage-reimport-has-art", "tracks", {"cover.jpg": "sq300.jpg", "front.png": "wide.png"}, "",
     [_IMP, ["import", "-A", "-q", "-L"], _LSA]),
    ("stage-reimport-art-gone", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{import: {{copy: no, move: yes}}}}",
     [_IMP, ["import", "-A", "-q", "-L"], _LSA]),
    ("files-fetchart-then-clearart", "tracks-art", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], import: {{write: yes}}, "
     "embedart: {{clearart_on_import: yes}}}}", [_IMP, _LSA]),
    ("files-clearart-then-fetchart", "tracks-art", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [embedart, fetchart], import: {{write: yes}}, "
     "embedart: {{clearart_on_import: yes}}}}", [_IMP, _LSA]),
    ("files-move-clearart", "tracks-art", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], import: {{write: yes, copy: no, move: yes}}, "
     "embedart: {{clearart_on_import: yes}}}}", [_IMP, _LSA]),
    # embedart: automatic, on art_set
    ("embed-auto", "tracks", {"cover.jpg": "sq300.jpg"}, _EMBED, [_IMP]),
    ("embed-auto-nowrite", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart]}}", [_IMP]),
    ("embed-auto-maxwidth", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], import: {{write: yes}}, embedart: {{maxwidth: 50}}}}",
     [_IMP]),
    ("embed-auto-remove", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], import: {{write: yes}}, "
     "embedart: {{remove_art_file: yes}}}}", [_IMP, _LSA]),
    ("embed-auto-off", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], import: {{write: yes}}, embedart: {{auto: no}}}}",
     [_IMP]),
    ("embed-fetchart-cmd", "tracks", {"front.png": "sq300.png"},
     "{{plugins: [embedart, fetchart], import: {{write: yes}}, fetchart: {{auto: no}}}}",
     [_IMP, ["fetchart"]]),
    ("clearart-on-import", "tracks-art", {},
     "{{plugins: [embedart], import: {{write: yes}}, embedart: {{clearart_on_import: yes}}}}",
     [_IMP]),
    ("no-clearart-on-import", "tracks-art", {}, "{{plugins: [embedart]}}", [_IMP]),
    # embedart commands
    ("embedart-album", "tracks", {"cover.jpg": "sq300.jpg"}, _EMBED.replace("write: yes",
                                                                           "write: no"),
     [_IMP, ["embedart", "-y"], ["embedart", "-y", "nomatch"]]),
    ("embedart-no-art", "tracks", {}, _EMBEDART, [_IMP, ["embedart", "-y"]]),
    ("embedart-file", "tracks", {}, _EMBEDART,
     [_IMP, ["embedart", "-y", "-f", "{src}/sq300.png", "title:One"]]),
    ("embedart-file-missing", "tracks", {}, _EMBEDART,
     [_IMP, ["embedart", "-y", "-f", "{src}/nope.jpg"]]),
    ("embedart-not-image", "tracks", {}, _EMBEDART,
     [_IMP, ["embedart", "-y", "-f", "{root}/config.yaml"]]),
    ("embedart-prompt", "tracks", {}, _EMBEDART,
     [_IMP, (["embedart", "-f", "{src}/sq300.jpg"], "n\n"),
      (["embedart", "-f", "{src}/sq300.jpg", "title:Two"], "\n"),
      (["embedart", "-f", "{src}/sq300.jpg"], "x\ny\n")]),
    ("embedart-album-prompt", "tracks", {"cover.jpg": "sq300.jpg"}, _EMBED.replace(
        "write: yes", "write: no"), [_IMP, (["embedart"], "n\n")]),
    ("embedart-maxwidth", "tracks", {}, "{{plugins: [embedart], embedart: {{maxwidth: 64, "
     "quality: 50}}}}", [_IMP, ["embedart", "-y", "-f", "{src}/big.jpg"]]),
    ("embedart-ifempty", "tracks", {}, "{{plugins: [embedart], embedart: {{ifempty: yes}}}}",
     [_IMP, ["embedart", "-y", "-f", "{src}/sq300.jpg"],
      ["embedart", "-y", "-f", "{src}/wide.png"]]),
    ("embedart-compare-reject", "tracks", {},
     "{{plugins: [embedart], embedart: {{compare_threshold: 5}}}}",
     [_IMP, ["embedart", "-y", "-f", "{src}/sq300.jpg"],
      ["embedart", "-y", "-f", "{src}/other.jpg"]]),
    ("embedart-compare-accept", "tracks", {},
     "{{plugins: [embedart], embedart: {{compare_threshold: 5}}}}",
     [_IMP, ["embedart", "-y", "-f", "{src}/sq300.jpg"],
      ["embedart", "-y", "-f", "{src}/sq600.jpg"]]),
    ("embedart-compare-no-art", "tracks", {},
     "{{plugins: [embedart], embedart: {{compare_threshold: 5}}}}",
     [_IMP, ["embedart", "-y", "-f", "{src}/other.jpg"]]),
    ("embedart-remove-art-file", "tracks", {"cover.jpg": "sq300.jpg"},
     "{{plugins: [fetchart, embedart], embedart: {{remove_art_file: yes}}}}",
     [_IMP, ["embedart", "-y"], _LSA]),
    ("embedart-url", "tracks", {}, _EMBEDART,
     [_IMP, ["embedart", "-y", "-u", "{url}/img/sq300.png"]]),
    ("embedart-url-errors", "tracks", {}, _EMBEDART,
     [_IMP, ["embedart", "-y", "-u", "{url}/img/nope.png"], ["embedart", "-y", "-u",
                                                             "{url}/page"]]),
    ("extractart", "tracks-art", {}, _EMBEDART,
     [_IMP, ["extractart", "-o", "{root}/out/x"], ["extractart", "-o", "{root}/out/y", "nomatch"],
      ["extractart", "-a"], _LSA]),
    ("extractart-name", "tracks-art", {}, _EMBEDART,
     [_IMP, ["extractart", "-a", "-n", "front"], ["extractart", "-n", "folder"],
      ["extractart", "-a", "-n", "sub/x"], _LSA]),
    ("extractart-none", "tracks", {}, _EMBEDART,
     [_IMP, ["extractart", "-o", "{root}/out/x"], ["extractart", "-a"], _LSA]),
    ("extractart-embeds", "tracks-art", {},
     "{{plugins: [embedart], import: {{write: yes}}}}",
     [_IMP, ["extractart", "-a", "-n", "front"], _LSA]),
    ("clearart", "tracks-art", {}, _EMBEDART,
     [_IMP, ["clearart", "-y", "title:One"], ["clearart", "-y"]]),
    ("clearart-prompt", "tracks-art", {}, _EMBEDART,
     [_IMP, (["clearart"], "n\n"), (["clearart", "title:Two"], "y\n")]),
]

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

_PLUGIN_LOAD = ("a plugin that fails to set up (bad `fetchart.sources`): beets logs \"** error "
                "loading plugin fetchart\" with a Python traceback and runs without it; beets-rs "
                "logs the same line with the error message (and the `fetchart` command fails "
                "with it)")
_NOT_ENABLED = ("a disabled plugin's command: beets doesn't know it (\"unknown command\"), "
                "beets-rs names the plugin to enable")
_EXTRACT_CRASH = ("extractart -o into a missing directory: beets crashes with a traceback "
                  "(FileNotFoundError), beets-rs reports the same error and exits 1")
EXPECTED.update({
    "art:sources-invalid:1": _PLUGIN_LOAD,
    "art:sources-invalid:state": _PLUGIN_LOAD,
    "art:sources-empty:1": _PLUGIN_LOAD,
    "art:sources-empty:state": _PLUGIN_LOAD,
    "art:cmd-not-enabled:1": _NOT_ENABLED,
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


# ---------------------------------------------------------------------------
# mbsync, in the import phase (against its fake MusicBrainz): a library
# imported from the fixtures, changed, then synced; each step is compared,
# and the ls/db/requests outcomes are taken after the last step.

_MBS = "plugins: [musicbrainz, mbsync]\n"
_REC = "00000000-0000-0000-0001-0000000000"
IMPORT_CASES += [
    ("mbsync-album", "tagged", ["-t"], _MBS, ["a"], [
        ["mbsync", "-p"],
        ["modify", "-y", "-M", "title:Alpha", "title=Wrong", "year=1999", "label=Other"],
        ["modify", "-y", "-M", "title:Beta", "artist=Somebody", "mb_trackid!"],
        ["mbsync", "-p"],
        ["mbsync"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    ("mbsync-album-nomove", "tagged", ["-t"], _MBS, ["a"], [
        ["modify", "-y", "-M", "-a", "album=Old Name"],
        ["mbsync", "-M", "-f", "$album|$title"],
        ["ls", "-p"],
    ]),
    ("mbsync-album-write", "tagged", ["-t"], _MBS + "import:\n    write: yes\n", ["a"], [
        ["modify", "-y", "-M", "-W", "title:Gamma", "title=Wrong"],
        ["mbsync", "-W", "-M"],
        ["modify", "-y", "-M", "-W", "title:Gamma", "title=Wrong"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    ("mbsync-skip", "fx", ["-A", "-C"], _MBS, [], [
        ["mbsync"],
        ["mbsync", "-f", "'%if{{$album,$album,$title}}'"],
    ]),
    ("mbsync-notfound", "tagged", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-a", "mb_albumid=99999999-9999-9999-9999-999999999999"],
        ["mbsync"],
        ["modify", "-y", "-a", "mb_albumid=not-an-id"],
        ["mbsync"],
    ]),
    ("mbsync-source", "tagged", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-M", "title:Alpha", "title=Wrong"],
        ["-P", "musicbrainz", "mbsync"],
        ["modify", "-y", "-a", "data_source=Discogs"],
        ["mbsync"],
    ]),
    ("mbsync-singletons", "fx", ["-A", "-s", "-C"], _MBS, [], [
        ["mbsync"],
        ["modify", "-y", "title:Alpha", f"mb_trackid={_REC}01"],
        ["modify", "-y", "title:Beta", f"mb_trackid={_REC}99"],
        ["mbsync", "-p"],
        ["ls", "-f", "$title|$artist|$mb_trackid|$length"],
        ["mbsync"],
        ["ls", "-f", "$title|$artist|$mb_trackid|$length"],
    ]),
    ("mbsync-va", "sampler", ["-t", "-C"], _MBS, ["a"], [
        ["modify", "-y", "-M", "disc:2", "title=Swapped", "artist=Nobody"],
        ["modify", "-y", "-M", "-a", "albumartist=Someone"],
        ["mbsync", "-p"],
        ["mbsync"],
    ]),
]
IMPORT_CASES += [
    # Files really move: a title and the album renamed (and moved) by
    # modify, then synced back.
    ("mbsync-album-move", "tagged", ["-t"], _MBS, ["a"], [
        ["modify", "-y", "title:Alpha", "title=Wrong"],
        ["modify", "-y", "-a", "album=Old Name"],
        ["ls", "-p"],
        ["mbsync"],
        ["ls", "-p"],
    ]),
    # Info listeners (ftintitle on albuminfo_received) apply to mbsync's
    # lookups too.
    ("mbsync-ftintitle", "sampler", ["-t", "-C"], "plugins: [musicbrainz, mbsync, ftintitle]\n",
     ["a"], [
        ["modify", "-y", "-M", "title:Ocean", "artist=Nobody", "title=Nothing"],
        ["mbsync", "-p"],
        ["mbsync"],
     ]),
]


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
