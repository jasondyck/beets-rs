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
beet mv -e -d /mnt/usb artist:radiohead            # export copies with library path formats (library unchanged)
beet update -F title -F artist                      # re-read changed tags (only these fields), drop deleted files
```

## What's implemented

| Area | Status |
|---|---|
| `import` | album and singleton (`-s`) modes; copy/move/in place (`-c/-C/-m/-M`); `link` (symlink), `hardlink`, `reflink: yes/no/auto` (auto falls back to copying), with beets' precedence (move, link, hardlink, reflink, then copy); `delete` (remove originals after copying); `ignore` globs and `ignore_hidden` when walking import directories; `-q` with `--quiet-fallback skip/asis`, `-t`/`timid`, `-a/-A`, `-w/-W`, `-S <mbid>`; `--pretend` (list files only); `--flat`; `--set field=value` / `set_fields` (templates); `-l/--log` / `log` (beets' log format); `none_rec_action`; `detail`; directory grouping with CD1/Disc 2 merging; `-g`/`group_albums` (split folders into albums by tags); duplicate detection for albums and singletons by `duplicate_keys` (`album`, `item`; exact field matches, flexible attributes too), with `duplicate_action` skip/merge/remove/keep/upgrade/ask (Upgrade imports only tracks that are new or have a higher bitrate than the old copy, adding them to the surviving old album), `duplicate_verbose_prompt`; `bell`; `singleton_album_disambig`; reimporting (by path or `-L/--library` query: rows replaced, files moved rather than copied inside the library, `added`/flexible attributes/art kept); `--from-scratch`; incremental imports (`-i`/`-I`, `-R`/`-r` for `incremental_skip_later`); resuming interrupted imports (`resume: ask/yes/no`, `-p/--resume`, `-P/--noresume`), with state in `statefile` (JSON) |
| Autotagger | MusicBrainz release and recording search, 1 req/s rate limit; beets' weighted distance (artist, album, year, label, catalog #, track title/artist/length/index/id, missing/unmatched tracks); Hungarian item↔track assignment; `import.languages` / `ignored_alias_types` / `musicbrainz.aliases_as_credits` (preferred MusicBrainz aliases for artist names, sort names and titles); strong/medium/low recommendation with `max_rec` caps |
| Prompts | Apply, More candidates, Skip, Use as-is, as Tracks, Group albums, Enter search, enter Id, aBort (singletons: Skip, Use as-is, Enter search, enter Id, aBort); candidate list; per-track change display |
| Queries | substring, `field:`, `:=` / `:=~` exact, `::regex`, numeric/duration/date ranges (`..`), bools, `path:`, `-`/`^` negation, `,` OR, `field+`/`field-` sort, flexible attributes |
| Templates | `$f`, `${f}`, escapes, `%lower %upper %capitalize %title %left %right %if %ifdef %asciify %initial %first %time %aunique` |
| Paths | `paths:` with `default`/`singleton`/`comp`/query keys, `replace:` rules, `asciify_paths`, `max_filename_length`, unique-path suffixes, empty-dir pruning (directories holding only `clutter` files count as empty) |
| Commands | `import` (`imp`, `im`), `list` (`ls`), `modify` (`mod`), `remove` (`rm`), `move` (`mv`), `update` (`upd`, `up`), `write`, `stats`, `fields`, `config`, `version`, `help [COMMAND]` (`?`), with beets' aliases; no command shows the help, and an unknown one gives beets' `unknown command 'x'` error |
| `modify` | as beets: `field=value` (values are templates, parsed by field type with beets' leniency: unparsable numbers become 0), `field!` (drops a flexible attribute, resets a fixed field), query terms otherwise (anything with `:` before the `=`); `-a` with album changes inherited by the items (fixed fields and flexible attributes, set or deleted; `-I/--noinherit` to keep them on the album), `-m/-M` (default: `import.move` or `import.copy`), `-w/-W` (default `import.write`), `-f/--format`, `-y`, `-l/--limit`; beets' change listing ("Modifying N items.", `field: old -> new` with difflib-style highlighting) and the "Really modify, move and write tags? (Yes/no/select)" prompt with per-object selection; `update` shows changes the same way |
| `update` | as beets: re-reads files whose mtime is newer than the stored one, shows changes the same way, and stores them; `-a`, `-m/-M` (default: `import.move` or `import.copy`; only files inside the library move), `-p/--pretend`, `-F/--field` (repeatable: only these fields), `-e/--exclude-field` (repeatable), `-f/--format`, `-l/--limit`; missing files are listed as "deleted" and removed (an emptied album goes too, with its art); albums take their fields from their first item again, and pass changes on to their items; asks before going on when the library directory is missing |
| `write` | as beets: writes tags only to files whose tags differ from the database, listing the differences (`field: file -> db`); `-p/--pretend`, `-f/--force` (write every file), `-l/--limit`; missing files are reported and skipped |
| `remove` | as beets: lists the items (with `-d`: `$path - $title`; with `-a`: grouped by album) and asks "Really remove N items [and M albums] from the library? (Yes/no/select)" or "Really DELETE N files [and M albums]? (Yes/no/select)", with per-object selection; `-a`, `-d/--delete` (deletes the files and album art, pruning emptied directories), `-f/--force`, `-l/--limit`; an album left empty is removed |
| Global options | before the command, as in beets: `-l/--library`, `-d/--directory`, `-c/--config`, `-v/--verbose` (repeatable: `-v` logs config/library paths, plugin loading, MusicBrainz searches and lookups, duplicates found; `-vv` also each MusicBrainz request), `-p/--plugins LIST` and `-P/--disable-plugins LIST` (comma-separated; also the `disabled_plugins` config key), `-h/--help`, `--version` |
| Tags | MP3, FLAC, M4A/ALAC, Ogg/Opus, WavPack, APE, AIFF, WAV via lofty |
| Release preferences | `match.preferred.countries` / `media` (regex lists, earlier = better) and `original_year`, as in beets |
| Plugins | enabled with `plugins:`; import hooks + subcommands. **fetchart** (folder image, then Cover Art Archive; `beet fetchart [-f]`), **lyrics** (LRCLIB, plain or synced; `beet lyrics [-p] [-f]`), **embedart** (embeds the album art on import, always after fetchart; `beet embedart [-f IMAGE] [-y]`, `beet extractart [-o PATH] [-a] [-n NAME]`, `beet clearart [-y]`; options `ifempty`, `remove_art_file`) |
| `move` | as beets: consolidates into the library directory, or `-d/--dest DIR`; `-c/--copy` (copy and point the library at the copies), `-e/--export` (copy, library unchanged), `-p/--pretend` (beets' `Source -> Destination` listing, two lines per file when it doesn't fit), `-t/--timid` ("Really move? (Yes/no/select)"), `-a`, `-l/--limit`; "Moving N items (M already in place)."; taken destinations get beets' `.1`, `.2` suffixes; missing files are skipped; emptied library directories are pruned (`clutter` ignored) |
| Album art | `artpath` follows the album on `mv`/`modify`/`update` (copied, and the copy recorded, with `mv -c`; copied into the export with `mv -e -a`, as beets, but not with item exports), and is deleted by `rm -d` |

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
`paths` · `duplicates` (duplicate keys, Upgrade, prompt summaries) · `fileops` (copy/move/link/reflink, pruning, ignore globs) · `media` (lofty tags) · `mb` (MusicBrainz) · `autotag` · `importer` ·
`commands` · `diff` (change highlighting) · `logging` (`-v`) · `state` (import resume/history) · `ui` · `plugins/` (`fetchart`, `embedart`, `lyrics`).
