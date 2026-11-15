# beets-rs

A Rust port of [beets](https://github.com/beetbox/beets), the music library
manager and MusicBrainz tagger.

- **Same commands, same syntax.** The binary is `beet`, and its commands,
  queries, path templates and config file work as in beets 2.14, so the
  [beets docs](https://beets.readthedocs.io/) apply.
- **Nearly all of beets' bundled plugins**, ported natively. No Python
  needed.
- **Checked against the real thing.** A differential test suite runs about
  4,000 cases through both beets and beets-rs and compares the results.
- **6–22x faster** than beets on a 10,000-track library (see
  [Performance](#performance)).
- **A desktop app, `beet-gui`,** for browsing, editing and importing (see
  [Desktop app](#desktop-app)).
- **Safe to try alongside beets.** beets-rs keeps its own config and library
  in `~/.config/beets-rs/`, and `beet migrate-beets` copies an existing beets
  library in without touching it.

## Install

Download the archive for your system from the
[latest release](https://github.com/jasondyck/beets-rs/releases/latest)
(Linux x86_64/ARM, macOS Apple silicon/Intel, Windows x86_64), unpack it,
and put `beet` (`beet.exe` on Windows) on your `PATH`. The archive also
holds the desktop app, `beet-gui`.

Or build from source with a recent Rust toolchain:

```sh
cargo build --release              # the binary is target/release/beet
cargo build --release -p beet-gui  # the desktop app, target/release/beet-gui
```

Building `beet-gui` on Linux needs the desktop development packages (on
Debian/Ubuntu: `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev
libxcb1-dev libx11-xcb-dev libfontconfig1-dev libfreetype-dev`).

## Quick start

```sh
beet import ~/Downloads/SomeAlbum              # tag and import an album, interactively
beet import -q -m ~/incoming                   # quiet: apply strong matches, skip the rest
beet ls artist:radiohead year:2000..2009       # query the library
beet ls -a -f '$albumartist - $album ($year)'  # albums, with a custom format
beet modify -a 'in rainbows' genre=Alternative # change fields
beet mv -e -d /mnt/usb artist:radiohead        # export copies, organised by your path formats
beet update                                    # pick up tag changes made by other programs
```

A small config (`~/.config/beets-rs/config.yaml`):

```yaml
directory: ~/Music
plugins: fetchart embedart lyrics
match:
    preferred:
        countries: ['GB|UK', US]
        media: [CD, Digital Media|File]
fetchart:
    maxwidth: 1200
```

## Desktop app

`beet-gui` is a desktop front end built with [GPUI](https://gpui.rs). It
uses the same config and library as `beet`.

- **Albums and Tracks pages:** browse with beets queries, with cover art and
  a sortable track table. Edit fields, play, move, write tags, remove, and run
  the loaded plugins' commands on one item or a selection.
- **Import page:** every `beet import` option as a form.
- **Console:** runs any `beet` command, with buttons to answer its prompts.
- **Configuration page:** edits `config.yaml`.
- **Keyboard:** press F1 to list the shortcuts.

Anything that changes the library runs the real `beet` command line, so the
app behaves exactly like `beet`. See [gui/README.md](gui/README.md) (`GUI.md`
in the release archives).

## Coming from beets

Your beets config works as it is: copy it to `~/.config/beets-rs/config.yaml`
(or point `$BEETSRSDIR` somewhere else), removing any `library:` or
`statefile:` line that points at beets' own files. Then copy your library in
with `beet migrate-beets ~/.config/beets/library.db`; your beets library is
only read. [docs/MIGRATING.md](docs/MIGRATING.md) has the details, including
which external programs some plugins use.

beets-rs aims to behave exactly like beets 2.14. Where it doesn't, on
purpose or not yet, [docs/DIFFERENCES.md](docs/DIFFERENCES.md) lists it.
The main gaps: Python `pluginpath` plugins (beets' Python plugin API).

## What's included

- **Import and autotagging**: MusicBrainz matching with beets' scoring and
  prompts, singletons, copy/move/link/reflink, duplicate handling,
  incremental and resumable imports.
- **Library**: beets' SQLite schema, query language (fields, regex, ranges,
  dates, sorting), path templates and template functions, multi-valued
  fields, flexible attributes.
- **Tags**: MP3, FLAC, M4A/ALAC, Ogg Vorbis/Opus/Speex, WavPack, APE,
  Musepack, AIFF, WAV and ASF/WMA, with beets' field mapping.
- **Commands**: `import`, `list`, `modify`, `update`, `write`, `move`,
  `remove`, `stats`, `config`, `fields`, `version`, `migrate-beets` and every
  plugin's commands.

**Plugins** (enable them with `plugins:` in your config):

| Kind | Plugins |
|---|---|
| Metadata sources | musicbrainz (default), discogs, spotify, deezer, beatport, tidal, chroma, mbpseudo |
| Art and lyrics | fetchart, embedart, lyrics, thumbnails, freedesktop |
| Tag cleanup | ftintitle, lastgenre, scrub, zero, the, titlecase, rewrite, advancedrewrite, substitute, fromfilename, albumtypes, bucket, inline, types, parentwork |
| Audio analysis | replaygain, keyfinder, autobpm, bpm, acousticbrainz, absubmit |
| Library tools | duplicates, replace, missing, edit, info, export, random, fish, mbsync, mbsubmit, convert, badfiles, unimported, importadded, importsource, importfeeds, filefilter, ihate, hook, permissions |
| Queries | fuzzy, bareasc, limit, loadext |
| Playlists and playback | play, playlist, smartplaylist, bpd, web, aura, ipfs |
| Media servers and services | mpdupdate, mpdstats, plexupdate, kodiupdate, embyupdate, subsonicupdate, subsonicplaylist, sonosupdate, mbcollection, listenbrainz, lastimport, metasync |
| Deprecated in beets | bpsync |

[docs/FEATURES.md](docs/FEATURES.md) describes every command, option and
plugin in detail.

## Performance

Times for beets 2.14.1 and beets-rs on the same synthetic 10,000-track
library (1,000 albums), with autotagging and file copying off. Each time is
the median of several runs:

| Command (10,000 tracks) | beets | beets-rs | Speedup |
|---|--:|--:|--:|
| `version` (startup) | 0.22 s | 0.011 s | 21x |
| `import -A` | 45.2 s | 2.03 s | 22x |
| `ls` (all items) | 2.43 s | 0.20 s | 12x |
| `ls -a` (all albums) | 0.29 s | 0.018 s | 16x |
| `ls artist:'Artist 7'` | 0.27 s | 0.018 s | 15x |
| `ls title::'^Song 1'` (regex) | 0.48 s | 0.037 s | 13x |
| `ls year:1970..1979` | 0.59 s | 0.045 s | 13x |
| `ls -f` (custom template) | 2.50 s | 0.20 s | 12x |
| `ls year- title+` (sorted) | 2.41 s | 0.19 s | 13x |
| `stats` | 1.25 s | 0.20 s | 6x |
| `mv -p` (pretend, all items) | 9.51 s | 0.71 s | 13x |
| `modify -y album:'Album 3' genre=Ska` | 1.16 s | 0.087 s | 13x |

Measured on Linux with an 8-core AMD Ryzen 7 3800X, a release build and warm
caches. Imports that autotag spend most of their time waiting on
MusicBrainz, which allows about one request per second, so the gain there is
smaller. `tools/bench/bench.sh` reruns the comparison (see
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#benchmarks)).

## Contributing

[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) covers building and testing, the
differential test suite against beets, how to add a plugin, and the source
layout. [PORTING.md](PORTING.md) is the work log and to-do list.

## License

MIT, like beets (see [LICENSE](LICENSE)). Third-party code and data that
beets-rs ports or embeds are credited in [NOTICE.md](NOTICE.md).
