//! beets' `beet help CMD`/`beet CMD -h` texts (optparse's layout).
//!
//! Taken once from reference beets 2.14.1's own parsers (each Subcommand's
//! optparse usage and options, with the plugins loaded so their commands are
//! included); do not edit by hand. `beet CMD -h` is compared against
//! reference beets by the diffbeets cases in `tools/diffbeets/cases/help.py`.

/// One subcommand's help: optparse's usage line(s, already `beet`-
/// expanded) and its option table's rows (the option column, help).
pub struct SubcommandHelp {
    pub name: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
}

/// Every beets (2.14.1) subcommand's help, sorted by name.
pub static SUBCOMMAND_HELP: &[SubcommandHelp] = &[
    SubcommandHelp {
        name: "absubmit",
        usage: "Usage: beet absubmit [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --force", "re-download data when already present"),
            (
                "-p, --pretend",
                "pretend to perform action, but show only files which would be processed",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "acousticbrainz",
        usage: "Usage: beet acousticbrainz [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --force", "re-download data when already present"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "aura",
        usage: "Usage: beet aura [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-d, --debug", "use Flask debug mode"),
        ],
    },
    SubcommandHelp {
        name: "autobpm",
        usage: "Usage: beet autobpm [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --force", "Overwrite existing BPM values"),
            ("-q, --quiet", "Suppress message when item already has BPM"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "bad",
        usage: "Usage: beet bad [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-v, --verbose",
                "view results for both the bad and uncorrupted files",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "bareasc",
        usage: "Usage: beet [options]\nExample: beet bareasc -f '$album: $title' artist:beatles",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "bench_aunique",
        usage: "Usage: beet bench_aunique [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --profile", "performance profiling"),
        ],
    },
    SubcommandHelp {
        name: "bench_match",
        usage: "Usage: beet bench_match [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --profile", "performance profiling"),
            ("-i ID, --id=ID", "album ID to match against"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "bpd",
        usage: "Usage: beet bpd [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "bpm",
        usage: "Usage: beet bpm [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "bpsync",
        usage: "Usage: beet bpsync [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show all changes but do nothing"),
            ("-m, --move", "move files in the library directory"),
            ("-M, --nomove", "don't move files in library"),
            ("-W, --nowrite", "don't write updated metadata to files"),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "chromasearch",
        usage: "Usage: beet chromasearch [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            (
                "-s SEARCH, --search=SEARCH",
                "Fingerprint to search for (from the output of fpcalc -plain)",
            ),
            ("-c COUNT, --count=COUNT", "Number of items in result"),
            (
                "--full",
                "Don't stop searching once we found an exact match",
            ),
            ("-w, --write", "Write computed fingerprints to files"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "clearart",
        usage: "Usage: beet clearart [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-y, --yes", "skip confirmation"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "completion",
        usage: "Usage: beet completion [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "config",
        usage: "Usage: beet config [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-p, --paths",
                "show files that configuration was loaded from",
            ),
            (
                "-e, --edit",
                "edit user configuration with $VISUAL (or $EDITOR)",
            ),
            ("-d, --defaults", "include the default configuration"),
            ("-c, --clear", "do not redact sensitive fields"),
        ],
    },
    SubcommandHelp {
        name: "convert",
        usage: "Usage: beet convert [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show actions but do nothing"),
            (
                "-t THREADS, --threads=THREADS",
                "change the number of threads, defaults to maximum available processors",
            ),
            (
                "-r, --refresh",
                "reconvert if original file is newer than converted file",
            ),
            (
                "-k, --keep-new",
                "keep only the converted and move the old files",
            ),
            ("-d DEST, --dest=DEST", "set the destination directory"),
            (
                "-f FORMAT, --format=FORMAT",
                "set the target format of the tracks",
            ),
            ("-y, --yes", "do not ask for confirmation"),
            ("-l, --link", "symlink files that do not need transcoding."),
            (
                "-H, --hardlink",
                "hardlink files that do not need transcoding. Overrides --link.",
            ),
            (
                "-m PLAYLIST, --playlist=PLAYLIST",
                "create an m3u8 playlist file containing the converted files. The playlist file will be saved below the destination directory, thus PLAYLIST could be a file name or a relative path. To ensure a working playlist when transferred to a different computer, or opened from an external drive, relative paths pointing to media files will be used.",
            ),
            (
                "-F, --force",
                "force transcoding. Ignores no_convert, never_convert_lossy_files, and max_bitrate",
            ),
            ("-a, --album", "match albums instead of tracks"),
            ("--limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "deezerupdate",
        usage: "Usage: beet deezerupdate [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "duplicates",
        usage: "Usage: beet duplicates [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-c, --count", "show duplicate counts"),
            (
                "-C PROG, --checksum=PROG",
                "report duplicates based on arbitrary command",
            ),
            ("-d, --delete", "delete items from library and disk"),
            (
                "-F, --full",
                "show all versions of duplicate tracks or albums",
            ),
            (
                "-s, --strict",
                "report duplicates only if all attributes are set",
            ),
            (
                "-k KEY, --key=KEY",
                "report duplicates based on keys (use multiple times)",
            ),
            ("-M, --merge", "merge duplicate items"),
            ("-m DEST, --move=DEST", "move items to dest"),
            ("-o DEST, --copy=DEST", "copy items to dest"),
            (
                "-t TAG, --tag=TAG",
                "tag matched items with 'k=v' attribute",
            ),
            ("-r, --remove", "remove items from library"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "edit",
        usage: "Usage: beet edit [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f FIELD, --field=FIELD", "edit this field also"),
            ("--all", "edit all fields"),
            ("-a, --album", "match albums instead of tracks"),
        ],
    },
    SubcommandHelp {
        name: "embedart",
        usage: "Usage: beet embedart [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f PATH, --file=PATH", "the image file to embed"),
            ("-y, --yes", "skip confirmation"),
            ("-u URL, --url=URL", "the URL of the image file to embed"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "export",
        usage: "Usage: beet export [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            ("-l, --library", "show library fields instead of tags"),
            ("--append", "if should append data to the file"),
            (
                "-i INCLUDED_KEYS, --include-keys=INCLUDED_KEYS",
                "comma separated list of keys to show",
            ),
            (
                "-o OUTPUT, --output=OUTPUT",
                "path for the output file. If not given, will print the data",
            ),
            (
                "-f FORMAT, --format=FORMAT",
                "the output format: json|jsonlines|csv|xml",
            ),
            ("--limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "extractart",
        usage: "Usage: beet extractart [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-o OUTPATH", "image output file"),
            (
                "-n FILENAME",
                "image filename to create for all matched albums",
            ),
            ("-a", "associate the extracted images with the album"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "fetchart",
        usage: "Usage: beet fetchart [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --force", "re-download art when already present"),
            (
                "-q, --quiet",
                "quiet mode: do not output albums that already have artwork",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "fields",
        usage: "Usage: beet fields [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "fingerprint",
        usage: "Usage: beet fingerprint [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "fish",
        usage: "Usage: beet fish [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --noFields", "omit album/track field completions"),
            (
                "-e EXTRAVALUES, --extravalues=EXTRAVALUES",
                "include specified field *values* in completions",
            ),
            (
                "-o OUTPUT, --output=OUTPUT",
                "where to save the script. default: ~/.config/fish/completions",
            ),
        ],
    },
    SubcommandHelp {
        name: "freedesktop",
        usage: "Usage: beet freedesktop [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "ftintitle",
        usage: "Usage: beet ftintitle [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-d, --drop",
                "drop featuring from artists and ignore title update",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "help",
        usage: "Usage: beet help [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "import",
        usage: "Usage: beet import [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-c, --copy", "copy tracks into library directory (default)"),
            ("-C, --nocopy", "don't copy tracks (opposite of -c)"),
            ("-m, --move", "move tracks into the library (overrides -c)"),
            (
                "-M, --nomove",
                "don't move tracks into the library (overrides -m)",
            ),
            ("-w, --write", "write new metadata to files' tags (default)"),
            ("-W, --nowrite", "don't write metadata (opposite of -w)"),
            ("-a, --autotag", "infer tags for imported files (default)"),
            (
                "-A, --noautotag",
                "don't infer tags for imported files (opposite of -a)",
            ),
            ("-p, --resume", "resume importing if interrupted"),
            ("-P, --noresume", "do not try to resume importing"),
            ("-q, --quiet", "never prompt for input: skip albums instead"),
            (
                "--quiet-fallback=QUIET_FALLBACK",
                "decision in quiet mode when no strong match: skip or asis",
            ),
            (
                "-l LOG, --log=LOG",
                "file to log untaggable albums for later review",
            ),
            (
                "-s, --singletons",
                "import individual tracks instead of full albums",
            ),
            ("-t, --timid", "always confirm all actions"),
            ("-L, --library", "retag items matching a query"),
            ("-i, --incremental", "skip already-imported directories"),
            (
                "-I, --noincremental",
                "do not skip already-imported directories",
            ),
            (
                "-R, --incremental-skip-later",
                "do not record skipped files during incremental import",
            ),
            (
                "-r, --noincremental-skip-later",
                "record skipped files during incremental import",
            ),
            (
                "--from-scratch",
                "erase existing metadata before applying new metadata",
            ),
            ("--flat", "import an entire tree as a single album"),
            (
                "-g, --group-albums",
                "group tracks in a folder into separate albums",
            ),
            ("--pretend", "just print the files to import"),
            (
                "-S ID, --search-id=ID",
                "restrict matching to a specific metadata backend ID",
            ),
            (
                "--from-logfile=PATH",
                "read skipped paths from an existing logfile",
            ),
            (
                "--set=FIELD=VALUE",
                "set the given fields to the supplied values",
            ),
        ],
    },
    SubcommandHelp {
        name: "info",
        usage: "Usage: beet info [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            ("-l, --library", "show library fields instead of tags"),
            ("-s, --summarize", "summarize the tags of all files"),
            (
                "-i INCLUDED_KEYS, --include-keys=INCLUDED_KEYS",
                "comma separated list of keys to show",
            ),
            ("-k, --keys-only", "show only the keys"),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("--limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "ipfs",
        usage: "Usage: beet ipfs [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --add", "Add to ipfs"),
            ("-g, --get", "Get from ipfs"),
            ("-p, --publish", "Publish local library to ipfs"),
            ("-i, --import", "Import remote library from ipfs"),
            ("-l, --list", "Query imported libraries"),
            ("-m, --play", "Play music from remote libraries"),
            ("--limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "keyfinder",
        usage: "Usage: beet keyfinder [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "lastgenre",
        usage: "Usage: beet lastgenre [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show actions but do nothing"),
            ("-f, --force", "modify existing genres"),
            ("-F, --no-force", "don't modify existing genres"),
            (
                "-k, --keep-existing",
                "combine with existing genres when modifying",
            ),
            (
                "-K, --no-keep-existing",
                "don't combine with existing genres when modifying",
            ),
            (
                "-s SOURCE, --source=SOURCE",
                "genre source: artist, album, or track",
            ),
            ("-A, --items", "match items instead of albums"),
            ("-a, --albums", "match albums instead of items (default)"),
        ],
    },
    SubcommandHelp {
        name: "lastimport",
        usage: "Usage: beet lastimport [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "lbimport",
        usage: "Usage: beet lbimport [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-f PATH, --export-file=PATH",
                "path to a ListenBrainz data export .zip file (instead of fetching from the API)",
            ),
            (
                "--max=MAX_LISTENS",
                "maximum number of listens to fetch via the API (default: all). This option does not apply when importing a file via -f/--export-file.",
            ),
        ],
    },
    SubcommandHelp {
        name: "list",
        usage: "Usage: beet [options]\nExample: beet list -f '$album: $title' artist:beatles",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "lslimit",
        usage: "Usage: beet lslimit [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("--head=HEAD", ""),
            ("--tail=TAIL", ""),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "lyrics",
        usage: "Usage: beet lyrics [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --print", "print lyrics to console"),
            (
                "-r dir, --write-rest=dir",
                "write lyrics to given directory as ReST files",
            ),
            ("-f, --force", "always re-download lyrics"),
            (
                "--keep-synced",
                "skip items that already have synced lyrics",
            ),
            (
                "--no-keep-synced",
                "do not skip items that already have synced lyrics",
            ),
            ("-l, --local", "do not fetch missing lyrics"),
            ("--limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "mbsubmit",
        usage: "Usage: beet mbsubmit [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "mbsync",
        usage: "Usage: beet mbsync [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show all changes but do nothing"),
            ("-m, --move", "move files in the library directory"),
            ("-M, --nomove", "don't move files in library"),
            ("-W, --nowrite", "don't write updated metadata to files"),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "mbupdate",
        usage: "Usage: beet mbupdate [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-r, --remove", "Remove albums not in beets library"),
        ],
    },
    SubcommandHelp {
        name: "metasync",
        usage: "Usage: beet metasync [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show all changes but do nothing"),
            (
                "-s SOURCES, --source=SOURCES",
                "comma-separated list of sources to sync",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "missing",
        usage: "Usage: beet missing [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-c, --count", "count missing tracks per album"),
            ("-t, --total", "count total of missing tracks"),
            (
                "-a, --album",
                "show missing album releases for artist instead of tracks; Defaults to only releases of type 'album'",
            ),
            (
                "--release-types=RELEASE_TYPES",
                "comma-separated list of release types for missing albums (valid: nat, album, single, ep, broadcast, other, compilation, soundtrack, spokenword, interview, audiobook, live, remix, dj-mix, mixtape/street)",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "modify",
        usage: "Usage: beet modify [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-m, --move", "move files in the library directory"),
            ("-M, --nomove", "don't move files in library"),
            ("-w, --write", "write new metadata to files' tags (default)"),
            ("-W, --nowrite", "don't write metadata (opposite of -w)"),
            ("-a, --album", "match albums instead of tracks"),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-y, --yes", "skip confirmation"),
            (
                "-I, --noinherit",
                "when modifying albums, don't also change item data",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "move",
        usage: "Usage: beet move [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-d DIR, --dest=DIR", "destination directory"),
            ("-c, --copy", "copy instead of moving"),
            (
                "-p, --pretend",
                "show how files would be moved, but don't touch anything",
            ),
            ("-t, --timid", "always confirm all actions"),
            ("-e, --export", "copy without changing the database path"),
            ("-a, --album", "match albums instead of tracks"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "mpdstats",
        usage: "Usage: beet mpdstats [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "--host=HOST",
                "set the hostname of the server to connect to",
            ),
            (
                "--port=PORT",
                "set the port of the MPD server to connect to",
            ),
            (
                "--password=PASSWORD",
                "set the password of the MPD server to connect to",
            ),
        ],
    },
    SubcommandHelp {
        name: "parentwork",
        usage: "Usage: beet parentwork [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-f, --force",
                "re-fetch when parent work is already present",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "play",
        usage: "Usage: beet play [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-A ARGS, --args=ARGS",
                "add additional arguments to the command",
            ),
            ("-R, --randomize", "randomize the order of playlist entries"),
            ("-y, --yes", "skip the warning threshold"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "plexupdate",
        usage: "Usage: beet plexupdate [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --auth", "Authenticate and login to Plex"),
        ],
    },
    SubcommandHelp {
        name: "random",
        usage: "Usage: beet random [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-n NUMBER, --number=NUMBER", "number of objects to choose"),
            ("-e, --equal-chance", "each field has the same chance"),
            (
                "-t TIME, --time=TIME",
                "total length in minutes of objects to choose",
            ),
            (
                "--field=FIELD",
                "field to use for equal chance sampling (default: albumartist)",
            ),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-p PATH, --path=PATH",
                "print paths for matched items or albums",
            ),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "remove",
        usage: "Usage: beet remove [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-d, --delete", "also remove files from disk"),
            ("-f, --force", "do not ask when removing items"),
            ("-a, --album", "match albums instead of tracks"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "replace",
        usage: "Usage: beet replace [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "replaygain",
        usage: "Usage: beet replaygain [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-t THREADS, --threads=THREADS",
                "change the number of threads, defaults to maximum available processors",
            ),
            (
                "-f, --force",
                "analyze all files, including those that already have ReplayGain metadata",
            ),
            ("-w, --write", "write new metadata to files' tags"),
            ("-W, --nowrite", "don't write metadata (opposite of -w)"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "scrub",
        usage: "Usage: beet scrub [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-W, --nowrite", "leave tags empty"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "splupdate",
        usage: "Usage: beet splupdate [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-p, --pretend",
                "display query results but don't write playlist files.",
            ),
            (
                "-f FORMAT, --format=FORMAT",
                "print per-track log lines with custom format",
            ),
            (
                "-d PATH, --playlist-dir=PATH",
                "directory to write the generated playlist files to.",
            ),
            (
                "--dest-regen",
                "regenerate the destination path as 'move' or 'convert' commands would do.",
            ),
            (
                "--relative-to=PATH",
                "generate playlist item paths relative to this path.",
            ),
            (
                "--prefix=PREFIX",
                "prepend string to every path in the playlist file.",
            ),
            (
                "--forward-slash",
                "force forward slash in paths within playlists.",
            ),
            ("--urlencode", "URL-encode all paths."),
            (
                "--uri-format=URI_FORMAT",
                "playlist item URI template, e.g. http://beets:8337/item/$id/file.",
            ),
            (
                "--output=OUTPUT",
                "specify the playlist format: m3u|extm3u.",
            ),
        ],
    },
    SubcommandHelp {
        name: "spotify",
        usage: "Usage: beet spotify [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-m MODE, --mode=MODE",
                "\"open\" to open Spotify with playlist, \"list\" to print (default)",
            ),
            (
                "-f, --show-failures",
                "list tracks that did not match a Spotify ID",
            ),
        ],
    },
    SubcommandHelp {
        name: "spotifysync",
        usage: "Usage: beet spotifysync [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-f, --force", "re-download data when already present"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "stats",
        usage: "Usage: beet stats [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-e, --exact", "exact size and time"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "submit",
        usage: "Usage: beet submit [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "subsonicplaylist",
        usage: "Usage: beet subsonicplaylist [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-d, --delete",
                "delete tag from items not in any playlist anymore",
            ),
        ],
    },
    SubcommandHelp {
        name: "subsonicupdate",
        usage: "Usage: beet subsonicupdate [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "thumbnails",
        usage: "Usage: beet thumbnails [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            (
                "-f, --force",
                "force regeneration of thumbnails deemed fine (existing & recent enough)",
            ),
            (
                "--dolphin",
                "create Dolphin-compatible thumbnail information (for KDE)",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "tidal",
        usage: "Usage: beet tidal [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --auth", "Authenticate and login to Tidal"),
        ],
    },
    SubcommandHelp {
        name: "tidalsync",
        usage: "Usage: beet tidalsync <query> [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            (
                "-f, --force",
                "re-fetch popularity even if already present (default: False)",
            ),
            (
                "-w, --write",
                "write updated tags to media files (default: False)",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "titlecase",
        usage: "Usage: beet titlecase [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "unimported",
        usage: "Usage: beet unimported [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "update",
        usage: "Usage: beet update [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-a, --album", "match albums instead of tracks"),
            ("-f FORMAT, --format=FORMAT", "print with custom format"),
            ("-m, --move", "move files in the library directory"),
            ("-M, --nomove", "don't move files in library"),
            ("-p, --pretend", "show all changes but do nothing"),
            ("-F FIELDS, --field=FIELDS", "list of fields to update"),
            (
                "-e EXCLUDE_FIELDS, --exclude-field=EXCLUDE_FIELDS",
                "list of fields to exclude from updates",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "version",
        usage: "Usage: beet version [options]",
        options: &[("-h, --help", "show this help message and exit")],
    },
    SubcommandHelp {
        name: "web",
        usage: "Usage: beet web [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-d, --debug", "debug mode"),
        ],
    },
    SubcommandHelp {
        name: "write",
        usage: "Usage: beet write [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-p, --pretend", "show all changes but do nothing"),
            (
                "-f, --force",
                "write tags even if the existing tags match the database",
            ),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
    SubcommandHelp {
        name: "zero",
        usage: "Usage: beet zero [options]",
        options: &[
            ("-h, --help", "show this help message and exit"),
            ("-l LIMIT, --limit=LIMIT", "limit query results"),
        ],
    },
];

/// The help for a command name (its canonical name, not an alias).
pub fn find(name: &str) -> Option<&'static SubcommandHelp> {
    SUBCOMMAND_HELP.iter().find(|h| h.name == name)
}
