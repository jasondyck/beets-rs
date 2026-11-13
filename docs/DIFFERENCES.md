# Differences from beets 2.14

beets-rs aims to be a drop-in replacement for
[beets 2.14](https://github.com/beetbox/beets): the same commands, options,
config keys, query and template syntax, path formats, database schema and
plugin settings, and the same output where it can be. This file lists every
difference we know of, with the reason.

Each item was checked against reference beets 2.14.1 (see
[PORTING.md](../PORTING.md) for the comparison setup) or by running both
`beet`s, and the differential tests (`tools/diffbeets`) keep them from
silently changing: the differences they can observe have an `EXPECTED` entry
there with the same reason. Differences in the *environment* (external
programs, optional Python libraries) are in [MIGRATING.md](MIGRATING.md);
things that aren't ported yet are at the end and in
[PORTING.md](../PORTING.md).

## Command line and output

- **Help doesn't open the library**: `beet --help`, `beet help [CMD]` and
  `beet CMD -h` load the enabled plugins first (so their load-time warnings,
  such as `limit`'s deprecation notice, come on stderr as in beets) but not
  the library; beets opens the library before printing help, so on a library
  that needs migrations its stdout starts with `Created database backup at:`
  lines. Help text and layout (optparse's `Usage:` block and options table)
  match beets'.
- **`beet migrate-beets`** exists only in beets-rs (it imports a beets
  library; see [MIGRATING.md](MIGRATING.md)).
- **A plugin that fails to set up** logs beets' `** error loading plugin X`
  line with the error message; beets prints a Python traceback after it.
  Reason: no Python runtime. Both run without the plugin (its defaults still
  show in `beet config -d`, as in beets).
- **A plugin that fails to set up still has its commands** (it is in
  `plugins:`): beets, which registers the loaded plugins'
  commands only, says `unknown command 'replaygain'`; beets-rs prints the
  command's help (from the reference's parser) and runs it with the plugin's
  defaults. Reason: whether a plugin loads is only known after the full
  configuration is read, and the command line (and its help) is built from
  the `plugins:` setting first.
- **Regex syntax errors** in queries carry the engine's message
  (Python `re` vs fancy-regex); the `invalid query: '...': '...' is not a
  regular expression:` part is the same. Reason: different regex engines.
- **Two beets bugs are not reproduced**: sorting albums by an item field
  (`beet ls -a title+`) lists an album once per item in beets (a LEFT JOIN
  without GROUP BY) where beets-rs lists it once; sorting albums by a field
  both tables have (`year+`) crashes beets with SQLite's "ambiguous column
  name", where beets-rs sorts by the album's field.

## Import

- **The state file has the same name** (`statefile`, default
  `state.pickle`) but is JSON; beets' is a Python pickle. A beets pickle is
  logged at debug level and treated as empty, and is replaced with JSON the
  next time progress is saved. A task is keyed by its first path; beets keys
  it by all of a task's paths. Reason: no Python; the JSON is readable and
  inspectable.
- **`--noresume` (`-P`) discards saved progress**, so the import starts
  over cleanly; beets stops resuming but keeps the progress records. Reason:
  deliberate (a discarded resume should not leave stale progress behind).
- **`mv` is idempotent when destinations collide**: an item's own file
  counts as a free name, so re-running `mv` leaves it alone, while beets
  renames the colliding files again on every run (suffixes drift, e.g.
  `Same.1` → `Same.3`). Reason: deliberate; repeated `mv` shouldn't churn
  the library.
- `%aunique`/`%sunique` disambiguation across albums of one threaded import
  matches beets 2.14 (the earlier difference was fixed in PORTING 3.7).

## Library, config and tags

- **The database uses SQLite's WAL journal** (`library.db-wal` /
  `-shm` files appear beside it). Reason: better concurrency and crash
  safety; the schema and contents are beets 2.14's.
- **`langdetect` language guessing is not ported** (beets' optional
  dependency): `lyrics_language` stays unset and `lyrics.translate`'s
  `from_languages` never matches, exactly as beets without langdetect.
- An **instrumental lyrics marker** is left for beets' instrumental
  migration to blank rather than blanked first; the end state is the same.
- **A library made by an older beets-rs** (absolute paths, `genre` columns)
  is rebuilt in beets' layout once, after a
  `<library>-before-beets_layout.bak` backup. beets never sees such a
  database.
- **ASF/WMA tags are not supported** (lofty has no ASF reader), so such
  files aren't imported as audio.
- **An empty list field written to APEv2 removes the tag**, where beets
  writes an empty value; MediaFile reads both as "no value". Reason: beets'
  own `write -f` then reports the field as changed again, which beets-rs
  avoids.
- **`pluginpath` is accepted and ignored** (beets' Python plugin API is not
  ported; see below).
- The **state file's content and the differences in writing tags** are
  covered above; the `fields`, `stats`, `config`, `modify`, `update`,
  `write`, `remove`, `move` output and exit statuses match beets in the
  differential tests.

## Queries, templates and paths

- **`%asciify` and `asciify_paths` use different Unicode tables**: the port
  uses `deunicode`'s with fixes for common CJK text, kana and vulgar
  fractions, so most text matches Python's `unidecode`, but some symbols
  differ (for example `™` becomes `tm`, where unidecode gives `(tm)`; the
  `bareasc` plugin applies a wider table of its own). Reason: unidecode's
  GPL tables aren't copied.
- Regex error messages as above; everything else in queries, sorts,
  templates and path formats is checked against beets case by case in
  `tools/diffbeets`.

## Album art

- **Images are processed in Rust** (the `image` and `jpeg-encoder` crates)
  instead of ImageMagick/PIL: resizing to a width (`1000x>`), JPEG quality
  (92; 4:2:0 subsampling below 90, as ImageMagick), a size limit by lowering
  the JPEG quality, reformatting, deinterlacing. Reason: no external
  programs needed for fetchart/embedart/`mv`.
- **`embedart.compare_threshold` similarity** reimplements ImageMagick's
  PHASH metric (moments of the blurred grayscale image) and is close to but
  not bit-identical with it (on beets' test images: 2.95 vs 2.26 and 11.2 vs
  10.1), so a borderline threshold can decide differently. Reason: no
  ImageMagick dependency.
- **`thumbnails` writes the PNG `Thumb::URI`/`Thumb::MTime` metadata with
  ImageMagick's `magick`** command (as beets does); the resizing itself is
  internal.

## Plugins: shared behaviour

- **The plugin event bus is per thread**: listeners run on the thread that
  sends an event, so files written by `convert`'s parallel transcoders don't
  reach `write`/`after_write` listeners (beets' registry is global). Reason:
  the registry is thread-local; the main import/command path behaves as
  beets.
- **A plugin's own error is reported, not raised as a traceback**: an
  invalid `permissions` mode, an `importadded` album with no recorded
  mtimes, or a `hook` command format error is logged and the run continues;
  beets crashes. Reason: no Python tracebacks.
- **`types`**: a field declared again with another type gives beets'
  message, not a traceback.
- **`inline`**: beets' Python expressions are evaluated by a built-in subset
  of Python 3 (no embedded Python): Python-only constructs (`import`,
  `lambda`, `def`, `class`, `try`, `with`, `global`, `del`, `assert`,
  `raise`, `yield`, starred expressions, set/dict comprehensions, most
  keyword arguments, modules) fail with a `NotImplementedError` message
  where beets runs them; integers are 64-bit; a runtime error ends the
  command with beets' `InlineError` message instead of a traceback. Reason:
  no embedded Python (decided in PORTING 5.14).
- **`limit`**: giving both `--head` and `--tail` is an error ("Only use one
  of --head and --tail"); beets raises a `ValueError` traceback.
- **`extractart -o` into a missing directory** is reported with beets'
  `FileNotFoundError` message and exit status 1; beets crashes with a
  traceback.
- **`filefilter`** matches paths as UTF-8; beets matches bytes. Reason: Rust
  strings; non-UTF-8 paths are replaced lossily.
- **`badfiles`** checks files in query order; beets uses `par_map`, whose
  order is arbitrary.
- **`replaygain`**: the `gstreamer` and `audiotools` backends are not ported
  (they need Python libraries), so selecting one fails to load the plugin
  with beets' error message. With the default `command` backend and an empty
  `command` the plugin fails to load too, as beets 2.14 does (same message,
  no traceback).
- **`metasync`**'s Amarok source talks D-Bus through `zbus` (pure Rust)
  instead of `dbus-python`; results are the same, and libdbus isn't needed.

## Plugins, one by one

- **acousticbrainz**: a URL missing at runtime is logged instead of raising,
  and the `Float(6)` fields format with beets' default one decimal: the port
  has no per-field float digits yet (PORTING 5.21e). Reason: no Python
  types.
- **absubmit**: analyses run one at a time (beets uses `par_map`); a
  transport error ends the command, as beets.
- **aura**: no Pillow, so image documents have no `width`/`height`; no
  flask-cors (the `cors` list is honoured for `Origin`); an invalid
  `page`/`limit` falls back to the default instead of crashing; the
  reference's threaded Flask server loses the library directory (relative
  paths, no `filesize`), which this port doesn't reproduce. Reason: no
  Python dependencies.
- **autobpm**: `beat_track_kwargs` only feeds librosa's beat tracker, which
  beets doesn't read (only the tempo), so they're accepted and ignored with
  a warning. The librosa 1.0.1 tempo pipeline (decode, `kaiser_fast`
  resampling, `onset_strength` with median aggregation, tempogram tempo with
  the log-normal prior) is reimplemented in Rust and gives the same
  estimates on the inputs compared (99/117 BPM click tracks, beets'
  `bpm.mp3` fixture at 117).
- **bpd**: `status`'s `elapsed` is the player's position (0 with the null
  player) and the duration falls back to the item's length; the single
  output is named `beets-rs`; `decoders` lists the formats beets-rs reads
  rather than GStreamer's; `kill` stops the server instead of exiting the
  process; `ctrl_profile` reports that guppy (a Python memory profiler) is
  missing; the optional `bpd-audio` (rodio) build doesn't compile yet
  (PORTING 5.20d). Reason: no GStreamer; audio output is optional (see
  [MIGRATING.md](MIGRATING.md)).
- **bareasc**: its own `unidecode` wrapper (a deunicode table with fixes for
  vulgar fractions and `™`) so its output matches Python's unidecode there.
- **bpsync** builds its Beatport client when the command runs, not while
  the plugin loads: without a `beatport_token.json` it asks for the OAuth
  token then (beets' `BPSyncPlugin.__init__` does it at load, so the plugin
  fails to load and `beet bpsync` is an unknown command). A Beatport lookup
  error (network, bad response) is logged at debug and skipped, where beets
  ends with a traceback; an item with an `mb_trackid` the release doesn't
  list is skipped too (beets raises `KeyError`).
- **chroma**: fingerprint POST bodies are sent plain rather than
  gzip-compressed. Reason: not done yet (no gzip in the request session).
- **convert**: a temporary transcode the library still uses (imported with
  `copy: no` or `link`) isn't deleted, where beets deletes it and leaves the
  library pointing at a missing file; a missing `tmpdir` or a failed tag
  write on import is a warning (beets crashes); the `after_convert` event
  isn't sent (no ported plugin listens to it, but `hook` can't see it).
- **duplicates**: a list key (`-k genres`) groups by the list (beets fails);
  a `-C` program that can't start is an error (beets crashes); a `tiebreak`
  field that doesn't exist sorts as empty (beets fails); a checksum is
  stored as text (beets stores a blob).
- **importfeeds**: `beet config -d` shows `relative_to` empty (and after the
  other keys) where beets adds it at run time as the library directory, in
  confuse's load order (PORTING 5.20c).
- **ipfs**: `-m/--play` isn't wired to the ported `play` plugin yet, and
  reports that instead of playing.
- **loadext**: beets-rs' bundled SQLite is built without extension loading,
  so enabling the plugin only logs beets' "does not support extensions"
  warning. Reason: statically linked SQLite with extensions disabled; whether
  to enable `rusqlite`'s `load_extension` is still open (PORTING 5.20c).
- **lyrics**: no `langdetect` language detection (see above).
- **mbpseudo**: the merged info keeps `data_source` `MusicBrainz`; beets
  switches the source after the match (the importer doesn't send beets'
  `album_matched` event yet; PORTING 5.21e).
- **mpdupdate / plexupdate / kodiupdate / subsonicupdate / mbcollection /
  listenbrainz / lastimport**: `mpd.host`/`port` take their values from the
  environment only while they still hold their defaults (so `beet config -d`
  shows the static defaults); `plex.ignore_cert_errors` is accepted but
  certificate verification can't be turned off (ureq); last.fm requests
  carry pylast's parameters but not its XML-identical headers; the
  ListenBrainz playlist helpers beets carries but no command uses aren't
  ported.
- **mpdstats**: `MPD_HOST`/`MPD_PORT` environment defaults aren't read
  (config and the command's options are), and `beet config -d` shows
  `mpd.music_directory` empty where beets fills in the library directory at
  run time (PORTING 5.21e).
- **random**: the shuffle uses this port's own RNG instead of Python's
  `random`; both are unseeded, so selections differ run to run.
- **smartplaylist**: `auto` is accepted but not wired up, as in beets 2.14,
  whose `database_change` listener never regenerates anything.
- **tidal**: the browser opener for `beet tidal --auth` is best effort (its
  failures are ignored, as beets' `webbrowser.Error`); everything else,
  including the 429 `Retry-After` handling, is as beets.
- **web**: keys are sorted in list responses too (beets' order there is
  arbitrary); a PATCHed flexible attribute shows as stored (a string); no
  Flask debugger or reloader with `-d`; no `ETag`/conditional requests for
  files; the startup banner is shortened. Reason: no Flask.
- Every other ported plugin behaves as beets 2.14 in the differential
  tests, with no known difference: advancedrewrite, albumtypes, bpm, bucket,
  edit, embedart, embyupdate, export, fetchart, fish, freedesktop,
  fromfilename, ftintitle, fuzzy, ihate, importsource, info, keyfinder,
  lastgenre, mbsubmit, mbsync, missing, parentwork, play, playlist,
  rewrite, scrub, sonosupdate, subsonicplaylist, substitute, the, thumbnails,
  titlecase, unimported and zero.

## Not ported yet

These aren't differences by design, just gaps (see [PORTING.md](../PORTING.md)
for the detail):

- **beets' developer-only `bench` plugin** (`bench_aunique`) isn't ported.
- **Python `pluginpath` plugins** (beets' Python plugin API) are out of
  scope; the bundled plugins are ported natively.
- **PORTING 5.20c loose ends**: `importfeeds`' computed `relative_to` in
  `config -d`, `loadext`'s extension loading, and the missing diffbeets cases
  for `importfeeds`/`metasync`.
- **PORTING 5.20d**: the `bpd-audio` build doesn't compile yet (a `?` on
  rodio's `try_seek` error); `beet bpd` runs with the null player meanwhile.
- **PORTING 5.21e loose ends**: `mpdstats`' environment defaults and computed
  `music_directory`, per-field float digits for `acousticbrainz`'s `Float(6)`
  fields, and `mbpseudo`'s post-match `data_source` switch (it needs beets'
  `album_matched` event).
- **`lyrics` language detection** (`langdetect`), **ASF/WMA tags**, and the
  small gaps noted above: `chroma` gzip bodies, `ipfs --play`, the
  ListenBrainz playlist helpers, `after_convert`, `bpd`'s `ctrl_profile`
  (guppy).
