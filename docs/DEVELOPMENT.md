# Developing beets-rs

```sh
cargo build                     # debug build
cargo test                      # unit and CLI tests
cargo clippy --all-targets      # must stay warning-free
cargo fmt
```

Some tests compare against reference beets and skip themselves when it isn't
installed: they look for `$BEETS_REFERENCE`, else
`~/.cache/beets-reference/beet` (a wrapper around a beets 2.14 install).

## Differential testing

`tools/diffbeets/diffbeets.py` builds the same library with reference beets
and with beets-rs (generated audio, same config) and compares `ls`, `ls -a`,
`-f` templates and `mv -p` destinations over ~4000 cases (beets' own tests,
plugin commands and template fields, tag round trips, import scenarios against
a fake MusicBrainz, art fetching/embedding and autobpm's tempi), runs the same
cases on a beets-rs library made with `migrate-beets` from reference beets'
library, then has each tool write its tags and compares the files as
MediaFile reads them (and both tools' reading of the files beets wrote). Each
known, deliberate difference is listed with its reason in
`tools/diffbeets/cases/`. See [`tools/diffbeets/README.md`](../tools/diffbeets/README.md).

## Adding a plugin

A plugin registers itself from its own file (see any file under `src/plugins/`)
with an `inventory::submit!` of `crate::plugin_registry::PluginDef`, so adding
one touches only its own new files, a section in `docs/FEATURES.md` and its
diffbeets cases.
`build.rs` scans `src/plugins/` and generates the module list in
`$OUT_DIR/plugin_modules.rs` (included by `plugins/mod.rs`, with absolute
`#[path]`s), so a new file (or `name/mod.rs` directory) is compiled without
editing any central list:

```rust
inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "myplugin",            // `plugins:` name
        order: 100,                  // order in beets' bundled-plugin list
        defaults: r#"myplugin:
    setting: 1
"#,                                  // settings the plugin's __init__ adds
        config_keys: &["myplugin"],  // top-level keys of those defaults
        redacted: &[],               // dotted keys `beet config` hides
        make: Some(registry_make),   // the plugin constructor
        fatal: false,                // a setup error ends the command
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "myplugin",      // the plugin the command checks/enables
            build: build_mycommand,  // fn() -> clap::Command
            run: run_mycommand,      // fn(&Config, &Library, &[Box<dyn Plugin>], &ArgMatches)
        }],
    }
}
```

`order` in `PluginDef` is the plugin's position in beets' bundled-plugin
list (it orders the "available:" names and the walk over enabled plugins): a
plain number picked per plugin, leave gaps between them (e.g. ten apart) and
take the next one for a new plugin; `plugin_orders_are_unique` fails on a
duplicate, which would otherwise depend on link order. Commands need no such
number: `beet --help` lists the core and plugin commands sorted by name, as
beets does.

`plugins::load`, the configuration defaults (`beet config -d`) and the plugin
subcommands on `beet`'s command line are built from this registry; the core
commands stay in `main.rs`. A plugin command takes the same clap arguments it
would in beets (derive `clap::Args` and `augment_args` on a `Command`), and
`run` does the work. Add tests next to the plugin (`src/plugins/..._tests.rs`,
`mod tests` at the bottom), a section in `docs/FEATURES.md` (plugins are
sorted alphabetically) and its name in the README's plugin list, and its cases as a module under `tools/diffbeets/cases/`
(auto-discovered; definitions such as `CASES`, `ART_CASES`, `IMPORT_CASES`,
`EXPECTED` and `ORDER` are picked up).

## Layout

`model` (typed fields + flex attrs) · `library` (SQLite, beets' schema and migrations) · `migrate` (`migrate-beets`) · `query` · `template` ·
`paths` · `duplicates` (duplicate keys, Upgrade, prompt summaries) · `fileops` (copy/move/link/reflink, pruning, ignore globs) · `media` (reading/writing items' files, art) · `mediafile` (port of MediaFile's tag mapping) · `audioprops` (length, bitrate and MP3 encoder fields from the format specifications) · `mb` (MusicBrainz) · `autotag` · `importer` ·
`artresizer` (image operations) · `commands` · `diff` (change highlighting) · `logging` (`-v`) · `events` (the plugin event bus) · `state` (import resume/history) · `ui` · `plugins/` (the bundled plugins, which register themselves; see [FEATURES.md](FEATURES.md)) · `requests` (HTTP session).

## Benchmarks

`tools/bench/bench.sh [n-files] [workdir]` builds the same synthetic library
with reference beets and with beets-rs and times common commands; the
README's Performance table comes from it.

`cargo bench --bench core` runs the criterion micro-benchmarks in
`benches/core.rs` (template parsing/rendering, query parsing and matching,
library queries over a 10,000-item database, path destinations, the
autotagger's distances). Filter by name (`cargo bench --bench core --
library`) and compare against a saved run with `--save-baseline before`
then `--baseline before`.
