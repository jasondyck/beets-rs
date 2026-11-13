"""Diffbeets cases: listings."""

ORDER = 20

EXTRA_CONFIGS = {}

from ._corpus import ROOT

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


def _build_cases():
    # Defined by the plugin areas, which are imported after this module.
    from .art_plugins import DUP_ARGS
    from .config import _config_cases
    from .art_plugins import TYPES_ARGS

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
    # inline
    _inl = ["-c", f"{ROOT}/inline.yaml"]
    out += [
        ("inline ls -f items", [*_inl, "ls", "-f", "$initial|$disc_and_track|$minsec|$decade|$kbps|"
                                "$genre1|$fname|$shout|$half|$later|$nothing|$glist|$compf|"
                                "$single|$words|$padded|$kind|$sortname"]),
        ("inline ls -a -f albums", [*_inl, "ls", "-a", "-f",
                                    "$album|$n|$total_len|$first|$tracks|$alb_total|$avg_br"]),
        ("inline ls -f album fields from items", [*_inl, "ls", "-f", "$title|$n|$first"]),
        ("inline mv -p", [*_inl, "mv", "-p"]),
        ("inline fields", [*_inl, "fields"]),
        ("inline query", [*_inl, "ls", "-f", "$title|$kind", "kind:multi"]),
        ("inline query album", [*_inl, "ls", "-a", "n:3"]),
        ("inline sort", [*_inl, "ls", "-f", "$minsec $title", "minsec+", "title+"]),
        ("inline %if", [*_inl, "ls", "-f", "%if{$later,later,first}|%upper{$initial}"]),
        ("inline ok field with syntax error", ["-c", f"{ROOT}/inline-syntax.yaml", "ls", "-f",
                                                "$ok", "track:1"]),
    ]
    # 5.18a
    for name, extra in [("rewrite.yaml", []), ("advancedrewrite.yaml", [])]:
        _c = ["-c", f"{ROOT}/{name}"]
        out += [
            (f"{name} ls", [*_c, "ls"]),
            (f"{name} ls -a", [*_c, "ls", "-a"]),
            (f"{name} ls -f", [*_c, "ls", "-f", "$artist|$albumartist|$artists|$title|$album|"
                               "$genres|$artist_sort"]),
            (f"{name} ls -a -f", [*_c, "ls", "-a", "-f", "$albumartist|$album|$genres"]),
            (f"{name} mv -p", [*_c, "mv", "-p"]),
            (f"{name} query", [*_c, "ls", "artist:Rewritten", ",", "artist:Artist"]),
            (f"{name} sort", [*_c, "ls", "-f", "$artist $title", "artist+", "title+"]),
        ]
    _s = ["-c", f"{ROOT}/substitute.yaml", "ls", "-f"]
    out += [
        ("substitute", [*_s, "%substitute{$artist}|%substitute{$title}|%substitute{}|"
                        "%substitute{a,b}"]),
        ("substitute paths", ["-c", f"{ROOT}/substitute.yaml", "mv", "-p"]),
    ]
    for name in ["bucket.yaml", "bucket-extra.yaml"]:
        out.append((f"{name}", ["-c", f"{ROOT}/{name}", "ls", "-f",
                                "%bucket{$year}|%bucket{$artist}|%bucket{$albumartist}|"
                                "%bucket{$original_year,year}|%bucket{$title}"]))
    for name in ["rewrite-bad.yaml", "advancedrewrite-bad.yaml", "advancedrewrite-list.yaml",
                 "bucket-bad.yaml"]:
        out.append((f"{name} load", ["-c", f"{ROOT}/{name}", "ls", "-f", "$artist", "title:Hit One"]))
    out.append(("bucket errors", ["-c", f"{ROOT}/bucket.yaml", "ls", "-f",
                                  "%bucket{$title,year}|%bucket{}|%bucket{a,b,c}|"
                                  "%bucket{$genres}", "title:Hit"]))
    _err = ["-c", f"{ROOT}/inline-errors.yaml", "ls", "-f"]
    for name in ["boom", "noname", "lam", "imp"]:
        out.append((f"inline error {name}", [*_err, f"$title ${name}", "title:Hit One"]))
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
    return out + _config_cases()


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

CASES = _build_cases
