# Migrating from beets

beets-rs is a Rust rewrite of beets 2.14 that speaks the same language: the
same commands and options, the same `config.yaml` keys, queries, templates and
path formats, and the same database schema. This guide covers switching over
without touching your beets installation. For the (short) list of differences,
see [DIFFERENCES.md](DIFFERENCES.md).

## 1. Install

Build the binary with a recent stable Rust toolchain:

```sh
git clone <this repository> beets-rs && cd beets-rs
cargo build --release            # the binary is target/release/beet
# or: cargo install --path .     # installs `beet` into ~/.cargo/bin
```

No Python, ImageMagick or GStreamer is needed for the core and for most
plugins; a few plugins call external programs (section 5) and `bpd`'s audio
output is an optional build feature (section 6).

## 2. Where beets-rs keeps its files

| What | Where |
|---|---|
| Configuration directory | `$BEETSRSDIR`, else `~/.config/beets-rs/` (on Linux; the platform's config directory elsewhere) |
| Configuration | `<config dir>/config.yaml` (same syntax and keys as beets'), plus `include:`d files and any `-c FILE` given on the command line |
| Library database | `library:` (default `library.db`, relative to the config dir) |
| Music directory | `directory:` (default `~/Music`), the root that beets-rs stores item paths relative to |
| Import state (`resume`, `incremental`) | `statefile:` (default `state.pickle`, relative to the config dir; JSON content, see below) |

**beets-rs keeps its own files in its own config directory**
(`~/.config/beets-rs`, not `~/.config/beets`) and only *reads* beets' files
for `beet migrate-beets` (read-only, see below) — as long as the config you
give it doesn't point `library:`/`statefile:` at beets' files (see below).
`beet config -p` lists the files it actually read, and `beet -c FILE` layers
another YAML file on top of the user config, as beets does.

The import state file has beets' default name but holds JSON: don't copy
beets' `state.pickle` into beets-rs' config directory. It is ignored if you do
(beets' pickles can't be read), and is replaced with JSON when progress is
next saved, so keep a backup if you want to go back to beets' incremental
state.

## 3. Bring your configuration over

```sh
mkdir -p ~/.config/beets-rs
cp ~/.config/beets/config.yaml ~/.config/beets-rs/config.yaml
$EDITOR ~/.config/beets-rs/config.yaml     # or: beet config -e
```

Most settings carry over unchanged; check these:

- **`library:` and `statefile:`** — if the copied config sets them to
  absolute paths inside beets' own directory
  (`/home/you/.config/beets/library.db`, `…/state.pickle`), beets-rs would
  use *beets'* files (and write to them!). Remove those lines or point them
  into beets-rs' config directory; the defaults (`library.db`,
  `state.pickle`, relative to the config dir) are what you want.
- `directory:` — keep it pointing at the same music root as beets so the
  migrated paths are right; relative paths resolve against the config
  directory.
- `pluginpath:` is accepted and ignored: beets' Python plugin API is not
  ported (the bundled plugins are native; see [DIFFERENCES.md](DIFFERENCES.md)
  for the two plugins that aren't).
- `plugins:` names are beets' names. All of beets 2.14's bundled plugins are
  available except the metadata sources `discogs`, `spotify`, `deezer` and
  `beatport` (so `bpsync` can't work) and the developer-only `bench` plugin
  (`bench_aunique`).
- `beet config -d` prints the effective configuration including the defaults
  of the enabled plugins, exactly as confuse prints it in beets.

## 4. Bring your library over

beets-rs' database is beets 2.14's schema, but **don't point beets-rs at
beets' `library.db`**: beets-rs would write to it (migrations, `added`/`mtime`
updates, imports). Instead, copy it into beets-rs' own database:

```sh
beet migrate-beets ~/.config/beets/library.db \
    --beets-config ~/.config/beets/config.yaml
```

- The beets database is opened **read-only and immutable** (SQLite's
  `mode=ro&immutable=1`): nothing is written to it or beside it. The command
  refuses to run while a non-empty `library.db-journal` or `library.db-wal` is
  present, so close beets (or let a running beets command finish) first.
- beets' `directory:` is taken from `--beets-config` (or `-d DIR`); it is only
  read. Paths are then stored relative to beets-rs' own `directory:` when
  inside it, so keep the two the same.
- Items, albums, their ids and flexible attributes are copied. beets'
  migrations that the source library hasn't had yet run on the copy, and it
  warns afterwards if the items' files don't exist (usually a wrong
  `directory:`).
- The beets-rs library must be empty; `-f/--force` replaces what it holds
  after backing it up to `library.db-before-migrate-beets.bak`.
- It is a one-way copy: beets keeps its own library and can keep being used.
  To re-run it later, use `-f` (or delete beets-rs' `library.db`).

Then check the result:

```sh
beet stats
beet ls -a | head
beet ls -f '$albumartist - $album ($year)' | head
beet fields            # the query/template fields available
```

beets-rs also runs beets' data migrations once on opening a library, so the
copy starts in beets 2.14's current format.

## 5. External programs

Nothing is needed for importing, listing, modifying or moving files: audio and
image work (tag reading/writing, length and bitrate, art resizing, `fetchart`,
`embedart`, `convert`'s copying, `bpd`'s protocol) is done in Rust. These
plugins call programs when installed/configured:

| Program | Needed by |
|---|---|
| `ffmpeg` | `convert` (beets' default format commands use it), `replaygain`'s `ffmpeg` backend |
| `magick` (ImageMagick 7) | `thumbnails` (writing the PNG thumbnail metadata; resizing is built in) |
| `fpcalc` (Chromaprint) | `chroma` (fingerprinting; `$FPCALC` overrides it) |
| `mp3val`, `flac` | `badfiles`' default checks (`badfiles.commands` overrides them) |
| `mp3gain`, `mp3rgain` or `aacgain`, `metaflac` | `replaygain`'s `command` and `metaflac` backends (the default backend is `command`, which needs `replaygain.command`, as in beets) |
| `KeyFinder` or `keyfinder-cli` | `keyfinder` |
| `ipfs` | `ipfs` |
| `streaming_extractor_music` (Essentia) | `absubmit` |
| Picard (with `mbsubmit.picard_path`) | `mbsubmit`'s "Open files with Picard" choice |
| your player (`play.command`), else `xdg-open`/`open` | `play` |
| `$VISUAL`/`$EDITOR` (or the `editor` setting, else `xdg-open`) | `config -e`, `edit` |

Plugins that talk to network services (MusicBrainz, Cover Art Archive,
LRCLIB, Last.fm, AcoustID, ListenBrainz, Subsonic, Emby, Plex, Kodi, Sonos,
TIDAL, IPFS gateways) need no extra programs. The service addresses and any
API keys are beets' config keys; see beets' documentation for them. `tidal`
logs in with `beet tidal --auth` (OAuth2 in a browser, the token saved to
`tidal.tokenfile`), as in beets.

`metasync`'s Amarok source uses D-Bus through the pure-Rust `zbus` crate, so
`dbus-python`/libdbus aren't needed.

## 6. Optional: audio output for `bpd`

`beet bpd` runs the MPD-protocol server without any audio output unless
built with the `bpd-audio` feature, which uses `rodio`/`cpal` (pure Rust, but
on Linux `cpal` links ALSA, so the ALSA development headers are needed at
build time, e.g. `libasound2-dev` on Debian/Ubuntu):

```sh
cargo build --release --features bpd-audio
```

**That build doesn't compile yet** (PORTING 5.20d: an error-type conversion in
`bpd/player.rs`'s seek); until it's fixed, build without the feature, or
expect to fix that one line.

Without the feature (or when no output device opens) the server still works
with a null player: clients can browse, queue and control, `status` reports
the state, but no sound plays, and it logs that on startup. `beet aura` and
`beet web` are plain HTTP servers and need nothing extra.

## 7. Going back

beets-rs never modifies beets' configuration, database or state, so going back
is simply using `beet` from beets again. Removing beets-rs is removing its
config directory (`~/.config/beets-rs` by default) and the binary;
`beet migrate-beets` made a copy of your beets library, not the other way
round.
