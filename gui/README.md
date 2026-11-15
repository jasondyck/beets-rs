# beet-gui

A desktop front end for beets-rs, built with [GPUI](https://gpui.rs) through
[GPUI Kit](https://gpui-kit.com) 0.9.

```sh
cargo run -p beet-gui --profile gui          # dependencies optimized, our code incremental
cargo build -p beet-gui --release            # for installing
beet-gui [-c CONFIG]… [-l LIBRARY] [-d DIRECTORY]   # BEETSRSDIR works as for beet
```

## Pages

- **Albums / Tracks**: browse the library with a beets query (`artist:x year:2000..2010 ^genre:rock`),
  cover art, a sortable track table, and a detail pane with every field.
  From the detail pane: edit fields (with or without moving files / writing tags), play (through
  the `play` plugin when it is loaded, albums too; otherwise the system's player), replace the
  audio file (`replace` plugin), show folder, update, write tags, move/copy/export, remove
  (optionally deleting files), one-click commands for the loaded plugins (fetchart, embedart,
  lyrics, autobpm, keyfinder, parentwork, ftintitle, spotifysync, …) and any command on the
  selection.
  "All matching" runs modify / update / write / move / remove over the whole query.
- **Selecting several**: Ctrl+Click adds or removes, Shift+Click (or Shift+↑/↓) selects a range,
  Ctrl+A selects the whole list, Esc keeps just the current row. With several selected the detail
  pane becomes a bulk panel: modify all, play, update, write tags, move, remove, plugin commands.
  The selection is passed to `beet` as one `id::^(1|2|…)$` query (an OR of thousands of terms is
  deeper than SQLite allows); with everything selected the search query is used instead.
- **Keyboard**: F1 lists every shortcut. Ctrl+1…6 switch pages, Ctrl+F or `/` searches (Enter
  moves to the list), ↑/↓/Home/End/PgUp/PgDn move, Enter opens an album's tracks or plays a track,
  Space plays, F2 edits (Ctrl+S saves, Esc cancels), Delete removes, Ctrl+U/T/M update, write
  tags and move, Ctrl+O shows the folder.
- **Import**: every `beet import` option as a form; the run happens in the console.
- **Console**: runs any `beet` command (plugin commands too). Output keeps its colours, and prompts
  (import candidates, `(Yes/no/select)` questions, numbered menus, …) get answer buttons; free-text
  prompts (plugin logins, search terms) are answered in the input below the output.
- **Commands**: common read-only and maintenance commands, and the loaded plugins' help.
- **Configuration**: edit `config.yaml` (YAML highlighting); saving reloads the library.

## How it works

- `backend.rs`: one thread owns the beets-rs `Library` (its configuration and query state are
  thread-local) and answers list/detail requests over channels.
- `runner.rs`: everything that changes the library runs the real command line: this executable
  started as `beet-gui --beet ARGS…`, which calls `beets_rs::run_cli_with`. So the GUI behaves
  exactly like `beet` (moves, tag writes, plugin events), and the CLI's prompts are answered from
  the UI over stdin.
- `app.rs`: the window.

Starting state can be set from the environment (for scripts, or to drive the GUI with no input
devices): `BEET_GUI_PAGE=albums|tracks|import|console|commands|config`, `BEET_GUI_QUERY=…`,
`BEET_GUI_SELECT=album:ID|item:ID[,ID…]`, `BEET_GUI_EDIT=1`, `BEET_GUI_SHORTCUTS=1`, `BEET_GUI_ACTION=modify|update|write|move|remove`,
`BEET_GUI_RUN='stats -e'`, `BEET_GUI_THEME=dark|light`.

## Notes

- On Wayland, GPUI only draws on the compositor's frame callbacks: while the screen is blanked
  (screensaver) the window doesn't repaint. For headless screenshots use a nested compositor,
  e.g. `dbus-run-session kwin_wayland --virtual --no-lockscreen --socket wl-test` and
  `WAYLAND_DISPLAY=wl-test`.
