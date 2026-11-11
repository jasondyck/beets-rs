# Porting beets to Rust: gap list

The goal is a full rewrite of [beets](https://github.com/beetbox/beets). Items
are ordered roughly by how much day-to-day beets users depend on them. Each is
checked off in the commit that lands it. Design rule throughout: beets'
syntax compatibility (queries, templates, config keys, command flags).

Already done before this list: import (album/singleton, copy/move/in place,
`-q -t -A -W -p -S`, disc merging, duplicates), MusicBrainz autotagger,
queries, templates, path formats, `ls modify rm mv update write stats fields
config`, plugins fetchart / lyrics / embedart, release preferences.

## 1. Core: import

- [x] **1.1 Resume and incremental imports**: `-i/-I`, `incremental_skip_later`, `resume: ask/yes/no`, `--resume/--noresume`, statefile. Differs from beets on purpose: `--noresume` discards saved progress, and singleton history is keyed by folder
- [x] **1.2 Missing prompt choices**: as Tracks (`T`), Group albums (`G`), `-g/--group-albums` / `import.group_albums`
- [x] **1.3 Import flags and options, part 1**: `--set field=value` / `import.set_fields`, `-l/--log` / `import.log`, `--flat`, `--quiet-fallback`, `import.none_rec_action`, `import.timid`, `import.detail`; `--pretend` lists files only. `-p` is now `--resume` and `-P` `--noresume`, as in beets
- [x] **1.4 Reimport**: `-L/--library` (import a library query), `--from-scratch`, preserving `added`/flex attrs on reimport, `duplicate_action: merge`
- [x] **1.5 File ops**: `import.link`, `hardlink`, `reflink`, `import.delete`; `ignore`, `ignore_hidden`, `clutter` (also used for empty-dir pruning)
- [x] **1.6 Duplicate options**: `import.duplicate_keys` (album/item), `duplicate_verbose_prompt`, `singleton_album_disambig`, `import.bell`, `import.languages`; duplicate check for singletons; beets' newer "Upgrade" duplicate action
- [x] **1.7 Singleton confirm flow**: picking a singleton candidate should show the change and ask Apply / More candidates, as beets does (ours applies immediately); the import header should list every path of a merged multi-disc album
- [x] **1.8 Grouping and defaults**: beets' `albums_in_dir` sibling multi-disc merging ("Album Disc 1" / "Album Disc 2" folders, collapse_paths incl. subfolders without audio); `import.default_action`

## 2. Core: commands and CLI

- [x] **2.1 Global CLI**: `version`, `help [cmd]`, beets' command aliases (`ls`, `rm`, `mv`, `up`/`upd`, `mod`, `imp`/`im`, `stat`?), `-v/-vv` verbosity (logging), `-p/--plugins` override, `--format-item`/`--format-album` not in beets (skip)
- [x] **2.2 modify**: `-f/--format`, `-I/--noinherit` (album→item field inheritance), `-m/-M`, `-w/-W`, `-a` album changes propagating to items, `field!` deletion, confirmation diff display as in beets
- [x] **2.3 move**: `-e/--export` (copy without updating db) vs `-c` (copy and update db), `-t/--timid`. Deliberate deviation: an item's own file counts as free for unique names, so repeated mv is idempotent
- [x] **2.4 update / write / remove**: `update -F/--field`, `-a`; `write -f/--force`; `remove` confirmation listing as in beets
- [x] **2.5 config**: `config -e/--edit` (`$VISUAL`/`$EDITOR`), `--clear`; `include:` files; `config -p` with `$BEETSRSDIR`; `-c/--config` should layer on top of the user config (repeatable) as in beets, not replace it
- [x] **2.6 ui**: `ui.color`, `ui.colors`, change-display highlighting as in beets, `terminal_width`, `ui.length_diff_thresh`

## 3. Core: queries, templates, library

- [ ] **3.1 Queries**: relative dates (`added:-1w..`, `-2d`, `-1y..`), item queries matching album-level fields and album flex attrs, `sort_case_insensitive`, regex case flags, `^`/`-` edge cases, `path:` with relative/`~` paths, `:` escaping in values
- [ ] **3.2 Templates**: `%sunique`; `$format`/`$bitrate`/`$samplerate` and other computed fields in path formats; `path_sep_replace`; `overwrite_null`
- [ ] **3.3 Tag field coverage**: every MediaFile field beets reads/writes (composer, lyricist, arranger, work, mb_workid, artist_sort/credit, albumartists multi-valued, genres, R128 gains, barcode, isrc, asin, script, language, etc.); `id3v23`; multi-valued fields
- [ ] **3.4 library.db compatibility**: schema identical to beets' (column names, types, path as BLOB, `added`/`mtime` floats, flex tables), so beets' own `library.db` format is readable and writable; plus a migration command (decided by the maintainer) that opens a beets `library.db` at a path the user passes **read-only** (SQLite `mode=ro`/`immutable=1`, never written) and copies items, albums and flex attributes into the beets-rs DB. Tests use synthetic beets-format DBs only; never point it at `~/.config/beets/library.db`
- [ ] **3.5 Performance**: push query filtering/sorting into SQL where possible (currently in-memory), so 50k+ item libraries stay fast; import pipeline threading (`threaded`)

## 4. Core: autotagger

- [ ] **4.1 Match options**: `match.ignored`, `match.required`, `ignored_media`, `ignore_data_tracks`, `ignore_video_tracks`, `track_length_grace/max` check, `distance_weights` completeness (source, tracks, etc.); `match.album_disambig_fields` / `singleton_disambig_fields`; penalty keys listed in insertion order
- [ ] **4.2 Metadata fidelity**: `artist_credit`, multi-artist/multi-value fields, album types, release group / work fields, `musicbrainz.extra_tags`, `musicbrainz.genres`, VA detection, pregap/data tracks, `per_disc_numbering` checks

## 5. Bundled plugins (by popularity)

Done: fetchart (filesystem + CAA), lyrics (LRCLIB), embedart.

- [ ] **5.1 fetchart/embedart completion**: image resizing (`maxwidth`, `quality`, `max_filesize`), `minwidth`, `enforce_ratio`, `cautious`, `cover_format`, `store_source`; more sources (itunes, amazon, albumart, wikipedia, fanarttv, lastfm); embedart `maxwidth`, `compare_threshold`
- [ ] **5.2 lastgenre**: Last.fm genre lookup with whitelist/canonicalization tree, `count`, `source`, `fallback`, `force`, `-a/-s`
- [ ] **5.3 chroma**: AcoustID fingerprinting via `fpcalc` (shell out), as an extra candidate source in import; `beet fingerprint`, `beet submit`
- [ ] **5.4 replaygain**: ffmpeg backend (track + album gain, R128 for Opus), `beet replaygain [-a] [-f]`, `auto`, `targetlevel`, `overwrite`
- [ ] **5.5 convert**: ffmpeg transcoding with `formats`, `format`, `dest`, `-k` keep new, `-t` threads, `-y`, `-p`, `no_convert`, `max_bitrate`, `auto` on import, `embed`, copy album art
- [ ] **5.6 scrub**: strip all tags and rewrite beets' metadata, `auto` on import
- [ ] **5.7 duplicates**: `beet duplicates [-a] [-c] [-k KEY] [-f FMT] [-d] [-m DEST] [-s] [-t TAG] [-p] [-F]`
- [ ] **5.8 ftintitle**: move "feat. X" from artist to title, `drop`, `format`, `auto`
- [ ] **5.9 edit**: edit metadata as YAML in `$EDITOR`, `--all`, `-f field`, `-e`; import prompt choices "edit" and "edit Candidates"
- [ ] **5.10 info**: `beet info [-l] [-k] [-i field] PATH|QUERY`
- [ ] **5.11 missing**: `beet missing [-c] [-t] [-a] [-f FMT]`
- [ ] **5.12 mbsync**: `beet mbsync [-p] [-M] [-W] [-f]` re-fetching MusicBrainz metadata by ID
- [ ] **5.13 the, zero**: `the` template function (`%the{}`, a/an options); `zero` (`fields`, `keep_fields`, per-field patterns, `update_database`, `beet zero`)
- [ ] **5.14 inline**: inline fields (`item_fields`, `album_fields`) via a small built-in expression language covering common inline uses (field access, string ops, conditionals, arithmetic), as decided by the maintainer; no embedded Python (no pyo3). Python `pluginpath` plugins are out of scope
- [ ] **5.15 smartplaylist**: m3u generation from queries, `playlist_dir`, `relative_to`, `prefix`, `auto` on db change, `beet splupdate`
- [ ] **5.16 web**: HTTP JSON API compatible with beets' web plugin (`/item/`, `/album/`, queries, `/item/<id>/file`), plus a minimal browser UI
- [ ] **5.17 types**: declare flex attribute types (int/float/bool/date) for queries and sorting
- [ ] **5.18 Smaller common plugins**: fromfilename, importadded, badfiles, random, permissions, hook, export, playlist, mbcollection, rewrite, substitute, bucket, ihate, filefilter, unimported, parentwork, albumtypes, advancedrewrite, mpdupdate, plexupdate, kodiupdate, subsonicupdate, listenbrainz/lastimport
- [ ] **5.19 Metadata source plugins**: discogs, spotify, deezer, beatport (as extra candidate sources)
- [ ] **5.20 Rarely used**: bpd, aura, play, keyfinder, autobpm, bpm, absubmit, mbsubmit, metasync, thumbnails, importfeeds, fish, loadext, fuzzy, bareasc, limit, titlecase

## 6. Quality

- [ ] **6.1 Integration tests**: CLI end-to-end tests in temp `BEETSRSDIR` with generated audio files (import → ls → modify → mv → update → rm)
- [ ] **6.2 README/docs**: keep README's "What's implemented" table current; document differences from beets

## Needs the maintainer

Nothing open.
