# beets-rs

A Rust port of the core of [beets](https://github.com/beetbox/beets), the music
library manager: import with MusicBrainz autotagging, a SQLite library, beets'
query language and path-format templates.

The binary is `beet`, and the query, template and config syntax follow beets,
so the [beets docs](https://beets.readthedocs.io/) mostly apply. The database
schema is its own. Config and library live in `~/.config/beets-rs/`
(override with `$BEETSRSDIR`) so a real beets `library.db` is never touched.

```sh
cargo build --release
beet import ~/Downloads/SomeAlbum        # interactive autotagging
beet import -q -m ~/incoming             # quiet: apply strong matches, skip the rest
beet import -i ~/Downloads               # incremental: skip folders imported before
beet ls artist:radiohead year:2000..2009 year-
beet ls -a -f '$albumartist - $album ($year)'
beet modify -a 'in rainbows' genre=Alternative
beet modify nude 'title=$title (live)' mood=calm   # values are templates; unknown fields become flexible attributes
beet mv -c -d /mnt/usb artist:radiohead            # export copies with library path formats
beet update                                         # re-read changed tags, drop deleted files
```

## What's implemented

| Area | Status |
|---|---|
| `import` | album and singleton (`-s`) modes; copy/move/in place (`-c/-C/-m/-M`); `-q` with `--quiet-fallback skip/asis`, `-t`/`timid`, `-a/-A`, `-w/-W`, `-S <mbid>`; `--pretend` (list files only); `--flat`; `--set field=value` / `set_fields` (templates); `-l/--log` / `log` (beets' log format); `none_rec_action`; `detail`; directory grouping with CD1/Disc 2 merging; `-g`/`group_albums` (split folders into albums by tags); duplicate detection (skip/merge/remove/keep); reimporting (by path or `-L/--library` query: rows replaced, files moved rather than copied inside the library, `added`/flexible attributes/art kept); `--from-scratch`; incremental imports (`-i`/`-I`, `-R`/`-r` for `incremental_skip_later`); resuming interrupted imports (`resume: ask/yes/no`, `-p/--resume`, `-P/--noresume`), with state in `statefile` (JSON) |
| Autotagger | MusicBrainz release and recording search, 1 req/s rate limit; beets' weighted distance (artist, album, year, label, catalog #, track title/artist/length/index/id, missing/unmatched tracks); Hungarian item↔track assignment; strong/medium/low recommendation with `max_rec` caps |
| Prompts | Apply, More candidates, Skip, Use as-is, as Tracks, Group albums, Enter search, enter Id, aBort (singletons: Skip, Use as-is, Enter search, enter Id, aBort); candidate list; per-track change display |
| Queries | substring, `field:`, `:=` / `:=~` exact, `::regex`, numeric/duration/date ranges (`..`), bools, `path:`, `-`/`^` negation, `,` OR, `field+`/`field-` sort, flexible attributes |
| Templates | `$f`, `${f}`, escapes, `%lower %upper %capitalize %title %left %right %if %ifdef %asciify %initial %first %time %aunique` |
| Paths | `paths:` with `default`/`singleton`/`comp`/query keys, `replace:` rules, `asciify_paths`, `max_filename_length`, unique-path suffixes, empty-dir pruning |
| Commands | `import ls modify rm mv update write stats fields config` |
| Tags | MP3, FLAC, M4A/ALAC, Ogg/Opus, WavPack, APE, AIFF, WAV via lofty |
| Release preferences | `match.preferred.countries` / `media` (regex lists, earlier = better) and `original_year`, as in beets |
| Plugins | enabled with `plugins:`; import hooks + subcommands. **fetchart** (folder image, then Cover Art Archive; `beet fetchart [-f]`), **lyrics** (LRCLIB, plain or synced; `beet lyrics [-p] [-f]`), **embedart** (embeds the album art on import, always after fetchart; `beet embedart [-f IMAGE] [-y]`, `beet extractart [-o PATH] [-a] [-n NAME]`, `beet clearart [-y]`; options `ifempty`, `remove_art_file`) |
| Album art | `artpath` follows the album on `mv`/`modify`/`update`, is copied on `mv -c -d`, and is deleted by `rm -d` |

## Example config

```yaml
directory: ~/Music
plugins: fetchart embedart lyrics
match:
    preferred:
        countries: ['GB|UK', US]
        media: [CD, Digital Media|File]
        original_year: yes
fetchart:
    maxwidth: 1200      # use Cover Art Archive's 1200px thumbnail
lyrics:
    synced: no
```

## Not yet

More plugins (replaygain, duplicates, web), image resizing, `%sunique`,
reading or writing beets' own `library.db`.

## Layout

`model` (typed fields + flex attrs) · `library` (SQLite) · `query` · `template` ·
`paths` · `media` (lofty tags) · `mb` (MusicBrainz) · `autotag` · `importer` ·
`commands` · `state` (import resume/history) · `ui` · `plugins/` (`fetchart`, `embedart`, `lyrics`).
