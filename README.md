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
| `import` | album and singleton (`-s`) modes; copy/move/in place (`-c/-m/-C`); `-q`, `-t`, `-A`, `-W`, `-p`, `-S <mbid>`; directory grouping with CD1/Disc 2 merging; duplicate detection (skip/keep/remove) |
| Autotagger | MusicBrainz release and recording search, 1 req/s rate limit; beets' weighted distance (artist, album, year, label, catalog #, track title/artist/length/index/id, missing/unmatched tracks); Hungarian item↔track assignment; strong/medium/low recommendation with `max_rec` caps |
| Prompts | Apply, More candidates, Skip, Use as-is, Enter search, enter Id, aBort; candidate list; per-track change display |
| Queries | substring, `field:`, `:=` / `:=~` exact, `::regex`, numeric/duration/date ranges (`..`), bools, `path:`, `-`/`^` negation, `,` OR, `field+`/`field-` sort, flexible attributes |
| Templates | `$f`, `${f}`, escapes, `%lower %upper %capitalize %title %left %right %if %ifdef %asciify %initial %first %time %aunique` |
| Paths | `paths:` with `default`/`singleton`/`comp`/query keys, `replace:` rules, `asciify_paths`, `max_filename_length`, unique-path suffixes, empty-dir pruning |
| Commands | `import ls modify rm mv update write stats fields config` |
| Tags | MP3, FLAC, M4A/ALAC, Ogg/Opus, WavPack, APE, AIFF, WAV via lofty |

## Not yet

Plugins (fetchart, lyrics, replaygain, …), import resume/incremental state,
"as Tracks" / "Group albums" prompt choices, preferred countries/media for
tie-breaking, album art, `%sunique`, reading or writing beets' own `library.db`.

## Layout

`model` (typed fields + flex attrs) · `library` (SQLite) · `query` · `template` ·
`paths` · `media` (lofty tags) · `mb` (MusicBrainz) · `autotag` · `importer` ·
`commands` · `ui`.
