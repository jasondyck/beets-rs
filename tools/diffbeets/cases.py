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
           ("aiff", "aiff", "1.37")], 1)]
    + [
        ("s03/pcm.wav", "wav", {"_d": "1.37"}),
        ("s01/lone1.mp3", "mp3", {"artist": "Solo Artist", "title": "Lone Track",
                                  "date": "2020"}),
        ("s01/lone2.mp3", "mp3", {"artist": "Solo Artist", "title": "Lone Track",
                                  "date": "2021"}),
        ("s01/star.mp3", "mp3", {"artist": "Solo Artist",
                                 "title": "What? Now: *star*", "date": "2020-07"}),
        ("s01/untagged.mp3", "mp3", {}),
        ("s02/stray.mp3", "mp3", {"artist": "Artist A", "title": "Stray Single",
                                  "album": "Album One", "date": "2001"}),
    ]
)

# Import steps: args to `beet`; the last one is a directory under src/.
IMPORTS = [
    ["import", "-A", "-q", d]
    for d in ["a01", "a02", "a03", "a04", "a05", "a06", "a07", "a08", "a09",
              "a10", "a11", "a12", "a13", "a14"]
] + [
    ["import", "-A", "-q", "-s", "s01"],
    ["import", "-A", "-q", "-s", "s02"],
    ["import", "-A", "-q", "-s", "s03"],
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
    # multi-valued (beets 2.14 lists) -- known gap
    "genres:rock",
    "genre:Rock",
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
                    "${length}|${bitrate}"]
STATS_FORMATS = ["MP3", "FLAC", "AAC", "ALAC", "OGG", "Opus", "WavPack", "AIFF", "WAVE"]

# `stats` options and queries (beets' show_stats; -l is beets' limit option).
STATS = [[], ["-e"], ["Hit"], ["-e", "Hit"], ["-l", "3"], ["-e", "-l", "3"],
         ["--limit", "1", "album:Lossless"], ["-l", "0"], ["-l", "100"],
         ["-l", "2", "title+"], ["-e", "-l", "4", "singleton:true"]]


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
    out.append(("mv -p", ["mv", "-p"]))
    out.append(("mv -p -d", ["mv", "-p", "-d", DEST]))
    out.append(("mv -p -a -d", ["mv", "-p", "-a", "-d", DEST]))
    out.append(("mv -p -d Hit", ["mv", "-p", "-d", DEST, "Hit"]))
    for name in ["nocaseins.yaml", "sortcfg.yaml"]:
        for q in SORT_CONFIG_QUERIES:
            args = _query_args(q)
            out.append((f"-c {name} ls:{q}", ["-c", f"{ROOT}/{name}", "ls", *args]))
            out.append((f"-c {name} ls -a:{q}", ["-c", f"{ROOT}/{name}", "ls", "-a", *args]))
    return out


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
_MULTI = ("multi-valued fields (beets 2.x `genres`, `artists`, ...; `genre` "
          "deprecated with a warning) are PORTING.md 3.3")
_ENCODER = "`encoder` (and other MediaFile fields) not read yet: PORTING.md 3.3"

EXPECTED = {
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
    "ls:genres:rock": _MULTI,
    "ls -a:genres:rock": _MULTI,
    "ls:genre:Rock": _MULTI,
    "ls -a:genre:Rock": _MULTI,
    "ls -f:$encoder": _ENCODER,
    "mv -p paths:$genre/$artist/$title": _MULTI,
}
