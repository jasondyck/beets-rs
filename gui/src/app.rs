//! The window: a sidebar of pages (albums, tracks, import, console,
//! commands, configuration) around the library worker and the `beet` runner.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use beets_rs::model::Kind;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, Theme, ThemeMode,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::backend::{
    self, Detail, Field, LibInfo, Opts, Req, Resp, Row, TOTALS_ONLY, split_words,
};
use crate::catalog;
use crate::runner::{self, Color as AnsiColor, Runner, Session};

actions!(
    beet_gui,
    [
        FocusSearch,
        Reload,
        ShowAlbums,
        ShowTracks,
        ShowImport,
        ShowPlugins,
        ShowConsole,
        ShowCommands,
        ShowConfig,
        ShowShortcuts,
        // The album / track list (key context `Library`).
        SelectAll,
        ClearSelection,
        CursorUp,
        CursorDown,
        CursorFirst,
        CursorLast,
        CursorPageUp,
        CursorPageDown,
        ExtendUp,
        ExtendDown,
        OpenSelection,
        PlaySelection,
        EditSelection,
        RemoveSelection,
        UpdateSelection,
        WriteSelection,
        MoveSelection,
        RevealSelection,
        // The field editor (key context `EditForm`) and the config page.
        SaveEdit,
        CancelEdit,
        SaveConfig,
    ]
);

/// Every shortcut: (keys, action, context, what it does). The help dialog
/// lists the ones with a description.
const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("Ctrl+F  /  Ctrl+L  /  /", "Search", "Anywhere"),
    (
        "Ctrl+1 … Ctrl+7",
        "Albums, Tracks, Import, Plugins, Console, Commands, Config",
        "Anywhere",
    ),
    ("F5", "Reload the library", "Anywhere"),
    ("F1  /  Ctrl+/", "This list", "Anywhere"),
    (
        "Enter (in search)",
        "Search now and move to the list",
        "Search",
    ),
    ("↑ ↓  Home End  PgUp PgDn", "Move through the list", "List"),
    ("Click", "Select one", "List"),
    ("Ctrl+Click", "Add to / remove from the selection", "List"),
    ("Shift+Click, Shift+↑ ↓", "Select a range", "List"),
    ("Ctrl+A", "Select everything in the list", "List"),
    ("Esc", "Select only the current row", "List"),
    ("Enter", "Album: show its tracks · Track: play", "List"),
    ("Space", "Play", "List"),
    (
        "F2  /  Ctrl+E",
        "Edit (several selected: modify them all)",
        "List",
    ),
    ("Delete", "Remove…", "List"),
    ("Ctrl+U", "Update from the files", "List"),
    ("Ctrl+T", "Write tags to the files", "List"),
    ("Ctrl+M", "Move / copy…", "List"),
    ("Ctrl+O", "Show the folder", "List"),
    ("Ctrl+S  /  Ctrl+Enter", "Save the edit", "Editing"),
    ("Esc", "Cancel the edit", "Editing"),
    ("Ctrl+S", "Save the configuration", "Configuration"),
];

pub fn bind_keys(cx: &mut App) {
    const LIST: Option<&str> = Some("Library");
    cx.bind_keys([
        KeyBinding::new("ctrl-f", FocusSearch, None),
        KeyBinding::new("ctrl-l", FocusSearch, None),
        KeyBinding::new("f5", Reload, None),
        KeyBinding::new("ctrl-1", ShowAlbums, None),
        KeyBinding::new("ctrl-2", ShowTracks, None),
        KeyBinding::new("ctrl-3", ShowImport, None),
        KeyBinding::new("ctrl-4", ShowPlugins, None),
        KeyBinding::new("ctrl-5", ShowConsole, None),
        KeyBinding::new("ctrl-`", ShowConsole, None),
        KeyBinding::new("ctrl-6", ShowCommands, None),
        KeyBinding::new("ctrl-7", ShowConfig, None),
        KeyBinding::new("f1", ShowShortcuts, None),
        KeyBinding::new("ctrl-/", ShowShortcuts, None),
        KeyBinding::new("/", FocusSearch, LIST),
        KeyBinding::new("ctrl-a", SelectAll, LIST),
        KeyBinding::new("escape", ClearSelection, LIST),
        // The track table binds Escape itself (to drop its cursor); ours
        // keeps the cursor and drops the rest of the selection.
        KeyBinding::new("escape", ClearSelection, Some("DataTable")),
        KeyBinding::new("up", CursorUp, LIST),
        KeyBinding::new("down", CursorDown, LIST),
        KeyBinding::new("home", CursorFirst, LIST),
        KeyBinding::new("end", CursorLast, LIST),
        KeyBinding::new("pageup", CursorPageUp, LIST),
        KeyBinding::new("pagedown", CursorPageDown, LIST),
        KeyBinding::new("shift-up", ExtendUp, LIST),
        KeyBinding::new("shift-down", ExtendDown, LIST),
        KeyBinding::new("enter", OpenSelection, LIST),
        KeyBinding::new("space", PlaySelection, LIST),
        KeyBinding::new("f2", EditSelection, LIST),
        KeyBinding::new("ctrl-e", EditSelection, LIST),
        KeyBinding::new("delete", RemoveSelection, LIST),
        KeyBinding::new("ctrl-u", UpdateSelection, LIST),
        KeyBinding::new("ctrl-t", WriteSelection, LIST),
        KeyBinding::new("ctrl-m", MoveSelection, LIST),
        KeyBinding::new("ctrl-o", RevealSelection, LIST),
        KeyBinding::new("ctrl-s", SaveEdit, Some("EditForm")),
        KeyBinding::new("ctrl-enter", SaveEdit, Some("EditForm")),
        KeyBinding::new("escape", CancelEdit, Some("EditForm")),
        KeyBinding::new("ctrl-s", SaveConfig, Some("ConfigPage")),
    ]);
}

/// The longest query argument to pass (Linux limits one argument to
/// 128 KiB).
const MAX_QUERY_ARG: usize = 120_000;

/// One query argument matching any of `ids` in `field` (`id`, `album_id`):
/// a regular expression, since an OR of thousands of terms is deeper than
/// SQLite allows.
fn ids_query(field: &str, ids: &[i64]) -> String {
    if let [id] = ids {
        return format!("{field}:{id}");
    }
    let alts: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!("{field}::^({})$", alts.join("|"))
}

/// Wait this long after the last keystroke before running a search.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(220);
/// How often the list is re-read while a command changes the library.
const LIVE_REFRESH: Duration = Duration::from_secs(2);

/// What a plugin command from the toolbox runs on.
#[derive(Clone, Copy, Debug)]
enum CommandTarget {
    Selection,
    Everything,
    Help,
    Console,
}

/// Where a keyboard move goes.
#[derive(Clone, Copy, Debug)]
enum MoveTo {
    First,
    Last,
    By(isize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Albums,
    Tracks,
    Import,
    Plugins,
    Console,
    Commands,
    Config,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::Albums,
        Page::Tracks,
        Page::Import,
        Page::Plugins,
        Page::Console,
        Page::Commands,
        Page::Config,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Albums => "Albums",
            Page::Tracks => "Tracks",
            Page::Import => "Import",
            Page::Plugins => "Plugins",
            Page::Console => "Console",
            Page::Commands => "Commands",
            Page::Config => "Configuration",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::Albums => IconName::BookOpen,
            Page::Tracks => IconName::Inbox,
            Page::Import => IconName::Plus,
            Page::Plugins => IconName::LayoutDashboard,
            Page::Console => IconName::PanelBottom,
            Page::Commands => IconName::Ellipsis,
            Page::Config => IconName::Asterisk,
        }
    }

    fn kind(self) -> Option<Kind> {
        match self {
            Page::Albums => Some(Kind::Album),
            Page::Tracks => Some(Kind::Item),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The track table.

const TRACK_COLUMNS: [(&str, &str, f32); 8] = [
    ("track", "#", 56.),
    ("title", "Title", 280.),
    ("artist", "Artist", 200.),
    ("album", "Album", 220.),
    ("year", "Year", 64.),
    ("length", "Length", 72.),
    ("format", "Format", 72.),
    ("genre", "Genre", 140.),
];

fn cell(row: &Row, col: usize) -> &str {
    match col {
        0 => &row.track,
        1 => &row.title,
        2 => &row.artist,
        3 => &row.album,
        4 => &row.year,
        5 => &row.length,
        6 => &row.format,
        _ => &row.genre,
    }
}

pub struct TrackTable {
    rows: Vec<Row>,
    /// The rows in the order the query returned them (for "no sort").
    original: Vec<Row>,
    /// The app's selection, for highlighting.
    marked: HashSet<i64>,
    /// A Ctrl/Shift click for the app to apply (the table leaves those
    /// alone, see `select_on_modifier_click`).
    pending_click: Option<(usize, Modifiers)>,
}

impl TrackTable {
    fn set_rows(&mut self, rows: Vec<Row>) {
        self.original = rows.clone();
        self.rows = rows;
    }
}

impl TableDelegate for TrackTable {
    fn columns_count(&self, _: &App) -> usize {
        TRACK_COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width) = TRACK_COLUMNS[col_ix];
        let col = Column::new(key, name).width(px(width)).sortable();
        if matches!(key, "year" | "length") {
            col.text_right()
        } else {
            col
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        match sort {
            ColumnSort::Default => self.rows = self.original.clone(),
            ColumnSort::Ascending | ColumnSort::Descending => {
                let numeric = matches!(col_ix, 4);
                self.rows.sort_by(|a, b| {
                    let (x, y) = (cell(a, col_ix), cell(b, col_ix));
                    if numeric {
                        x.parse::<i64>()
                            .unwrap_or(0)
                            .cmp(&y.parse::<i64>().unwrap_or(0))
                    } else if col_ix == 5 {
                        duration_secs(x).cmp(&duration_secs(y))
                    } else {
                        x.to_lowercase().cmp(&y.to_lowercase())
                    }
                });
                if sort == ColumnSort::Descending {
                    self.rows.reverse();
                }
            }
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let marked = self
            .rows
            .get(row_ix)
            .is_some_and(|r| self.marked.contains(&r.id));
        div()
            .id(("row", row_ix))
            .when(marked, |d| d.bg(selection_bg(cx)))
            .on_click(cx.listener(move |table, e: &ClickEvent, _, cx| {
                let m = e.modifiers();
                if m.secondary(cx) || m.shift {
                    table.delegate_mut().pending_click = Some((row_ix, m));
                    cx.notify();
                }
            }))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self
            .rows
            .get(row_ix)
            .map(|r| cell(r, col_ix).to_string())
            .unwrap_or_default();
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when(matches!(col_ix, 0 | 4 | 5 | 6), |d| {
                d.text_color(cx.theme().muted_foreground)
            })
            .child(text)
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        self.rows
            .get(row_ix)
            .map(|r| SharedString::from(cell(r, col_ix).to_string()))
    }
}

fn duration_secs(s: &str) -> i64 {
    s.split(':').fold(0, |acc, part| {
        acc * 60 + part.trim().parse::<i64>().unwrap_or(0)
    })
}

// ---------------------------------------------------------------------------
// The field editor.

struct EditForm {
    kind: Kind,
    id: i64,
    fields: Vec<(Field, Entity<InputState>)>,
    filter: Entity<InputState>,
    new_key: Entity<InputState>,
    new_value: Entity<InputState>,
    move_files: bool,
    write_tags: bool,
}

impl EditForm {
    /// `modify` arguments for what changed: `field=value`, `field!` for a
    /// cleared flexible attribute.
    fn changes(&self, cx: &App) -> Vec<String> {
        let mut out = vec![];
        for (field, input) in &self.fields {
            let new = input.read(cx).value().to_string();
            let old = field.edit.clone().unwrap_or_default();
            if new == old {
                continue;
            }
            if new.is_empty() && field.flex {
                out.push(format!("{}!", field.key));
            } else {
                out.push(format!("{}={new}", field.key));
            }
        }
        let key = self.new_key.read(cx).value().trim().to_string();
        if !key.is_empty() {
            out.push(format!("{key}={}", self.new_value.read(cx).value()));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Dialog forms for commands over the selection or the whole query.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Modify,
    Update,
    Write,
    Move,
    Remove,
}

impl Action {
    fn title(self) -> &'static str {
        match self {
            Action::Modify => "Modify",
            Action::Update => "Update from files",
            Action::Write => "Write tags",
            Action::Move => "Move / copy files",
            Action::Remove => "Remove",
        }
    }
}

struct ActionForm {
    action: Action,
    kind: Kind,
    /// The query the command runs on.
    query: Vec<String>,
    description: String,
    count: Option<usize>,
    assignments: Entity<InputState>,
    dest: Entity<InputState>,
    delete: bool,
    copy: bool,
    export: bool,
    pretend: bool,
    no_move: bool,
    no_write: bool,
    force: bool,
}

impl ActionForm {
    fn args(&self, cx: &App) -> Vec<String> {
        let album = self.kind == Kind::Album;
        let mut a: Vec<String> = vec![];
        let push = |a: &mut Vec<String>, s: &str| a.push(s.to_string());
        match self.action {
            Action::Modify => {
                push(&mut a, "modify");
                push(&mut a, "-y");
                if album {
                    push(&mut a, "-a");
                }
                if self.no_move {
                    push(&mut a, "-M");
                }
                if self.no_write {
                    push(&mut a, "-W");
                }
                a.extend(self.query.iter().cloned());
                a.extend(split_words(&self.assignments.read(cx).value()));
                return a;
            }
            Action::Update => {
                push(&mut a, "update");
                if album {
                    push(&mut a, "-a");
                }
                if self.no_move {
                    push(&mut a, "-M");
                }
                if self.pretend {
                    push(&mut a, "-p");
                }
            }
            Action::Write => {
                push(&mut a, "write");
                if self.force {
                    push(&mut a, "-f");
                }
                if self.pretend {
                    push(&mut a, "-p");
                }
            }
            Action::Move => {
                push(&mut a, "move");
                if album {
                    push(&mut a, "-a");
                }
                let dest = self.dest.read(cx).value().trim().to_string();
                if !dest.is_empty() {
                    push(&mut a, "-d");
                    a.push(dest);
                }
                if self.export {
                    push(&mut a, "-e");
                } else if self.copy {
                    push(&mut a, "-c");
                }
                if self.pretend {
                    push(&mut a, "-p");
                }
            }
            Action::Remove => {
                push(&mut a, "remove");
                push(&mut a, "-f");
                if album {
                    push(&mut a, "-a");
                }
                if self.delete {
                    push(&mut a, "-d");
                }
            }
        }
        // `write` matches items: an album's tracks are `album_id:N`.
        if self.action == Action::Write && album {
            a.extend(self.query.iter().map(|q| match q.strip_prefix("id:") {
                // `id:N` or the selection's `id::^(…)$`.
                Some(rest) => format!("album_id:{rest}"),
                None => q.clone(),
            }));
        } else {
            a.extend(self.query.iter().cloned());
        }
        a
    }
}

impl Render for ActionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let what = if self.kind == Kind::Album {
            "albums"
        } else {
            "tracks"
        };
        let target = match self.count {
            Some(n) => format!("{} ({n} {what})", self.description),
            None => self.description.clone(),
        };
        let everything = self.query.is_empty();
        let check = |id: &'static str,
                     label: &'static str,
                     value: bool,
                     set: fn(&mut ActionForm, bool),
                     cx: &mut Context<Self>| {
            Checkbox::new(id)
                .label(label)
                .checked(value)
                .on_click(cx.listener(move |this, v: &bool, _, cx| {
                    set(this, *v);
                    cx.notify();
                }))
        };
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Applies to: {target}")),
            )
            .when(everything && self.action != Action::Modify, |d| {
                d.child(
                    div()
                        .text_color(cx.theme().warning)
                        .child("The query is empty: this applies to the whole library."),
                )
            })
            .when(self.action == Action::Modify, |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child("Assignments (field=value, field! to delete)")
                        .child(Input::new(&self.assignments)),
                )
                .child(check(
                    "no-move",
                    "Don't move files",
                    self.no_move,
                    |f, v| f.no_move = v,
                    cx,
                ))
                .child(check(
                    "no-write",
                    "Don't write tags",
                    self.no_write,
                    |f, v| f.no_write = v,
                    cx,
                ))
            })
            .when(self.action == Action::Update, |d| {
                d.child(check(
                    "no-move",
                    "Don't move files",
                    self.no_move,
                    |f, v| f.no_move = v,
                    cx,
                ))
                .child(check(
                    "pretend",
                    "Pretend (show changes only)",
                    self.pretend,
                    |f, v| f.pretend = v,
                    cx,
                ))
            })
            .when(self.action == Action::Write, |d| {
                d.child(check(
                    "force",
                    "Write even if the tags match",
                    self.force,
                    |f, v| f.force = v,
                    cx,
                ))
                .child(check(
                    "pretend",
                    "Pretend (show changes only)",
                    self.pretend,
                    |f, v| f.pretend = v,
                    cx,
                ))
            })
            .when(self.action == Action::Move, |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child("Destination directory (empty: the library directory)")
                        .child(Input::new(&self.dest)),
                )
                .child(check(
                    "copy",
                    "Copy instead of moving",
                    self.copy,
                    |f, v| f.copy = v,
                    cx,
                ))
                .child(check(
                    "export",
                    "Export (copy, keep library paths)",
                    self.export,
                    |f, v| f.export = v,
                    cx,
                ))
                .child(check(
                    "pretend",
                    "Pretend (show moves only)",
                    self.pretend,
                    |f, v| f.pretend = v,
                    cx,
                ))
            })
            .when(self.action == Action::Remove, |d| {
                d.child(check(
                    "delete",
                    "Also delete the files from disk",
                    self.delete,
                    |f, v| f.delete = v,
                    cx,
                ))
                .when(self.delete, |d| {
                    d.child(
                        div()
                            .text_color(cx.theme().danger)
                            .child("Files will be deleted permanently."),
                    )
                })
            })
            .child(
                div()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("beet {}", runner::shell_join(&self.args(cx)))),
            )
    }
}

// ---------------------------------------------------------------------------
// The import form.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tri {
    Default,
    Yes,
    No,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileMode {
    Default,
    Copy,
    Move,
    InPlace,
}

struct ImportForm {
    paths: Entity<InputState>,
    search_id: Entity<InputState>,
    set: Entity<InputState>,
    mode: FileMode,
    write: Tri,
    incremental: Tri,
    autotag: bool,
    singletons: bool,
    group_albums: bool,
    flat: bool,
    timid: bool,
    quiet: bool,
    pretend: bool,
    from_scratch: bool,
}

impl ImportForm {
    fn args(&self, cx: &App) -> Vec<String> {
        let mut a = vec!["import".to_string()];
        let mut flag = |on: bool, f: &str| {
            if on {
                a.push(f.to_string());
            }
        };
        flag(self.mode == FileMode::Copy, "-c");
        flag(self.mode == FileMode::Move, "-m");
        flag(self.mode == FileMode::InPlace, "-C");
        flag(self.write == Tri::Yes, "-w");
        flag(self.write == Tri::No, "-W");
        flag(self.incremental == Tri::Yes, "-i");
        flag(self.incremental == Tri::No, "-I");
        flag(!self.autotag, "-A");
        flag(self.singletons, "-s");
        flag(self.group_albums, "-g");
        flag(self.flat, "--flat");
        flag(self.timid, "-t");
        flag(self.quiet, "-q");
        flag(self.pretend, "--pretend");
        flag(self.from_scratch, "--from-scratch");
        for id in split_words(&self.search_id.read(cx).value()) {
            a.push("-S".into());
            a.push(id);
        }
        for s in split_words(&self.set.read(cx).value()) {
            a.push("--set".into());
            a.push(s);
        }
        a.extend(split_words(&self.paths.read(cx).value()));
        a
    }
}

// ---------------------------------------------------------------------------
// Startup state from the environment: open on a page, with a query, a
// selection, a running command or an open form. Handy for scripts, and how
// the GUI is exercised without input devices (e.g. in a headless
// compositor).

#[derive(Default)]
struct Startup {
    page: Option<Page>,
    query: Option<String>,
    /// `album:N` or `track:N,M,…` (several: a multiple selection).
    select: Option<(Kind, Vec<i64>)>,
    run: Option<String>,
    edit: bool,
    action: Option<Action>,
    dark: Option<bool>,
    shortcuts: bool,
    plugin: Option<String>,
}

impl Startup {
    fn from_env() -> Option<Startup> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let s = Startup {
            page: var("BEET_GUI_PAGE").and_then(|p| {
                Page::ALL
                    .into_iter()
                    .find(|x| x.label().to_lowercase().starts_with(&p.to_lowercase()))
            }),
            query: var("BEET_GUI_QUERY"),
            select: var("BEET_GUI_SELECT").and_then(|v| {
                let (k, id) = v.split_once(':')?;
                let kind = match k {
                    "album" => Kind::Album,
                    "item" | "track" => Kind::Item,
                    _ => return None,
                };
                let ids: Vec<i64> = id.split(',').filter_map(|i| i.parse().ok()).collect();
                (!ids.is_empty()).then_some((kind, ids))
            }),
            run: var("BEET_GUI_RUN"),
            edit: var("BEET_GUI_EDIT").is_some(),
            action: var("BEET_GUI_ACTION").and_then(|a| {
                Some(match a.as_str() {
                    "modify" => Action::Modify,
                    "update" => Action::Update,
                    "write" => Action::Write,
                    "move" => Action::Move,
                    "remove" => Action::Remove,
                    _ => return None,
                })
            }),
            dark: var("BEET_GUI_THEME").map(|t| t == "dark"),
            shortcuts: var("BEET_GUI_SHORTCUTS").is_some(),
            plugin: var("BEET_GUI_PLUGIN"),
        };
        let any = s.page.is_some()
            || s.query.is_some()
            || s.select.is_some()
            || s.run.is_some()
            || s.edit
            || s.action.is_some()
            || s.dark.is_some()
            || s.shortcuts
            || s.plugin.is_some();
        any.then_some(s)
    }
}

// ---------------------------------------------------------------------------
// The application.

pub struct BeetsApp {
    backend: flume::Sender<Req>,
    /// Play the selection's file once its detail (with the path) arrives.
    play_when_loaded: bool,
    runner: Runner,
    page: Page,

    info: Option<LibInfo>,
    open_error: Option<String>,
    opening: bool,

    search: Entity<InputState>,
    search_gen: u64,
    list_gen: u64,
    loading: bool,
    list_error: Option<String>,
    /// The (kind, query) each list was last loaded with.
    albums_query: Option<String>,
    tracks_query: Option<String>,
    albums: Vec<Row>,
    album_scroll: UniformListScrollHandle,
    tracks: Entity<TableState<TrackTable>>,

    /// The current row (the detail pane shows it); with `marked`, the
    /// cursor of a multiple selection.
    selected: Option<(Kind, i64)>,
    /// The selection when more than the current row is selected (ids of
    /// `selected`'s kind, the current row included).
    marked: HashSet<i64>,
    /// Where a Shift selection starts (a row index in the shown list).
    anchor: Option<usize>,
    /// A row the app selected in the track table itself, whose `SelectRow`
    /// echo is not a user's click.
    syncing_row: Option<usize>,
    album_focus: FocusHandle,
    detail_gen: u64,
    detail: Option<Detail>,
    detail_error: Option<String>,
    edit: Option<EditForm>,
    plugin_cmd: Entity<InputState>,
    field_filter: Entity<InputState>,

    active_session: Option<usize>,
    console_input: Entity<InputState>,
    command_input: Entity<InputState>,
    console_scroll: ScrollHandle,
    follow_output: bool,

    import: ImportForm,

    /// The plugin toolbox.
    catalog: Vec<catalog::PluginInfo>,
    plugin_search: Entity<InputState>,
    /// A category chip, "Enabled", or None for everything.
    plugin_filter: Option<&'static str>,
    plugin_selected: Option<&'static str>,
    plugin_message: Option<(bool, String)>,

    config_editor: Entity<EditorState>,
    config_path: Option<PathBuf>,
    config_saved: String,
    config_message: Option<(bool, String)>,

    startup: Option<Startup>,
    /// Startup work waiting on the selection's details.
    startup_after_detail: Option<Startup>,

    _subscriptions: Vec<Subscription>,
}

impl BeetsApp {
    pub fn new(opts: Opts, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend::Backend { tx, rx } = backend::spawn(opts.clone());
        let runner = Runner::new(opts.cli_args());

        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Query, as for beet ls: artist:beatles year:1965..1970 ^genre:rock")
        });
        let plugin_cmd = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Command for this selection, e.g. lyrics -f")
        });
        let field_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter fields"));
        let console_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Answer, then Enter"));
        let command_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("beet command, e.g. ls -a year:2020 or help")
        });
        let import = ImportForm {
            paths: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Folders or files to import (quote paths with spaces)")
            }),
            search_id: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("MusicBrainz release/recording ids (optional)")
            }),
            set: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("field=value to set on everything imported (optional)")
            }),
            mode: FileMode::Default,
            write: Tri::Default,
            incremental: Tri::Default,
            autotag: true,
            singletons: false,
            group_albums: false,
            flat: false,
            timid: false,
            quiet: false,
            pretend: false,
            from_scratch: false,
        };
        let config_editor = cx.new(|cx| EditorState::new(window, cx).language("yaml"));
        let plugin_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search plugins: cover art, genre, lyrics, convert, playlist…")
        });
        let tracks = cx.new(|cx| {
            TableState::new(
                TrackTable {
                    rows: vec![],
                    original: vec![],
                    marked: HashSet::new(),
                    pending_click: None,
                },
                window,
                cx,
            )
            .row_selectable(true)
            .select_on_modifier_click(false)
            .col_selectable(false)
            .sortable(true)
            .col_resizable(true)
        });

        let mut subs = vec![];
        subs.push(cx.subscribe_in(
            &search,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => this.schedule_search(window, cx),
                InputEvent::PressEnter { .. } => {
                    this.run_search(cx);
                    this.focus_list(window, cx);
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &tracks,
            window,
            |this, table, ev: &TableEvent, _window, cx| {
                let row = |ix: usize, cx: &App| {
                    table
                        .read(cx)
                        .delegate()
                        .rows
                        .get(ix)
                        .map(|r| (r.id, r.album_id()))
                };
                match ev {
                    TableEvent::SelectRow(ix) => {
                        if this.syncing_row.take() == Some(*ix) {
                            return;
                        }
                        if row(*ix, cx).is_some() {
                            this.click_row(*ix, Modifiers::default(), cx);
                        }
                    }
                    TableEvent::DoubleClickedRow(ix) => {
                        if let Some((id, _)) = row(*ix, cx) {
                            this.select_only(Kind::Item, id, Some(*ix), cx);
                            this.play_selection(cx);
                        }
                    }
                    _ => {}
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &console_input,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    this.answer(&text, cx);
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &command_input,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    if this.run_typed_command(&text, cx) {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &plugin_cmd,
            window,
            |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    if this.run_on_selection(&text, cx) {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                    }
                }
            },
        ));
        // Ctrl/Shift clicks in the track table.
        subs.push(cx.observe(&tracks, |this, table, cx| {
            let click = table.update(cx, |t, _| t.delegate_mut().pending_click.take());
            if let Some((ix, m)) = click {
                this.click_row(ix, m, cx);
            }
        }));
        subs.push(cx.observe(&field_filter, |_, _, cx| cx.notify()));
        subs.push(cx.subscribe_in(
            &plugin_search,
            window,
            |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => cx.notify(),
                // Enter picks the first match.
                InputEvent::PressEnter { .. } => {
                    this.plugin_selected = this.shown_plugins(cx).first().map(|p| p.name);
                    cx.notify();
                }
                _ => {}
            },
        ));
        subs.push(cx.observe(&config_editor, |_, _, cx| cx.notify()));

        // The library worker's answers.
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(resp) = rx.recv_async().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_response(resp, window, cx);
                        cx.notify();
                        window.refresh();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Keep the list current while a command changes the library.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(LIVE_REFRESH).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        this.live_refresh(cx);
                        window.refresh();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Output from `beet` runs.
        let events = runner.rx.clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = events.recv_async().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        if let Some(done) = this.runner.handle(ev) {
                            this.on_session_finished(done, cx);
                        }
                        // Batch whatever else is already waiting.
                        for done in this.runner.drain() {
                            this.on_session_finished(done, cx);
                        }
                        if this.follow_output {
                            this.console_scroll.scroll_to_bottom();
                        }
                        cx.notify();
                        window.refresh();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let this = BeetsApp {
            backend: tx,
            runner,
            page: Page::Albums,
            info: None,
            open_error: None,
            opening: true,
            search,
            search_gen: 0,
            list_gen: 0,
            loading: false,
            list_error: None,
            albums_query: None,
            tracks_query: None,
            albums: vec![],
            album_scroll: UniformListScrollHandle::new(),
            tracks,
            selected: None,
            marked: HashSet::new(),
            anchor: None,
            syncing_row: None,
            album_focus: cx.focus_handle(),
            play_when_loaded: false,
            detail_gen: 0,
            detail: None,
            detail_error: None,
            edit: None,
            plugin_cmd,
            field_filter,
            active_session: None,
            console_input,
            command_input,
            console_scroll: ScrollHandle::new(),
            follow_output: true,
            import,
            catalog: catalog::plugins(),
            plugin_search,
            plugin_filter: None,
            plugin_selected: None,
            plugin_message: None,
            config_editor,
            config_path: None,
            config_saved: String::new(),
            config_message: None,
            startup: Startup::from_env(),
            startup_after_detail: None,
            _subscriptions: subs,
        };
        // Open once the first frame is up: a notify before the window is
        // attached schedules no repaint, so a fast answer would be lost.
        cx.defer_in(window, |this, _, _| {
            this.backend.send(Req::Open).ok();
        });
        this
    }

    // -- library --------------------------------------------------------

    fn query_text(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_gen = self.search_gen.wrapping_add(1);
        let generation = self.search_gen;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                if this.search_gen == generation {
                    this.run_search(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Load the current page's list for the search box's query.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.page.kind() else {
            return;
        };
        let query = self.query_text(cx);
        self.load(kind, query, cx);
    }

    fn load(&mut self, kind: Kind, query: String, cx: &mut Context<Self>) {
        self.loading = true;
        self.send_list(kind, query, cx);
    }

    /// While a command that changes the library runs (an import adds
    /// albums one by one), re-read the shown list and the totals.
    fn live_refresh(&mut self, cx: &mut Context<Self>) {
        if !self
            .runner
            .sessions
            .iter()
            .any(|s| s.mutates && s.running())
            || self.loading
        {
            return;
        }
        if let Some(kind) = self.page.kind() {
            let query = self.query_text(cx);
            self.send_list(kind, query, cx);
        } else {
            // No list shown: the totals come with any list, so ask for an
            // empty one under a generation that is never current.
            self.backend
                .send(Req::List {
                    generation: TOTALS_ONLY,
                    kind: Kind::Album,
                    query: "id:0".into(),
                })
                .ok();
        }
    }

    fn send_list(&mut self, kind: Kind, query: String, cx: &mut Context<Self>) {
        self.list_gen += 1;
        match kind {
            Kind::Album => self.albums_query = Some(query.clone()),
            Kind::Item => self.tracks_query = Some(query.clone()),
        }
        self.backend
            .send(Req::List {
                generation: self.list_gen,
                kind,
                query,
            })
            .ok();
        cx.notify();
    }

    /// Show a page, loading its list if the query changed since.
    fn show(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        let query = self.query_text(cx);
        match page {
            Page::Albums if self.albums_query.as_ref() != Some(&query) => {
                self.load(Kind::Album, query, cx)
            }
            Page::Tracks if self.tracks_query.as_ref() != Some(&query) => {
                self.load(Kind::Item, query, cx)
            }
            Page::Config => self.load_config_file(window, cx, false),
            _ => {}
        }
        cx.notify();
    }

    /// Everything may have changed: reopen and reload what's shown.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.opening = true;
        self.backend.send(Req::Open).ok();
        self.albums_query = None;
        self.tracks_query = None;
        if let Some(kind) = self.page.kind() {
            let q = self.query_text(cx);
            self.load(kind, q, cx);
        }
        if let Some((kind, id)) = self.selected {
            self.request_detail(kind, id);
        }
        cx.notify();
    }

    fn select(&mut self, kind: Kind, id: i64, cx: &mut Context<Self>) {
        if self.selected == Some((kind, id)) {
            return;
        }
        if self.selected.is_some_and(|(k, _)| k != kind) {
            self.marked.clear();
            self.anchor = None;
        }
        self.selected = Some((kind, id));
        self.edit = None;
        self.request_detail(kind, id);
        cx.notify();
    }

    /// The shown list's ids, in display order.
    fn list_ids(&self, kind: Kind, cx: &App) -> Vec<i64> {
        match kind {
            Kind::Album => self.albums.iter().map(|r| r.id).collect(),
            Kind::Item => self
                .tracks
                .read(cx)
                .delegate()
                .rows
                .iter()
                .map(|r| r.id)
                .collect(),
        }
    }

    /// The current row's index in the shown list.
    fn cursor(&self, cx: &App) -> Option<usize> {
        let (kind, id) = self.selected?;
        (self.page.kind() == Some(kind))
            .then(|| self.list_ids(kind, cx).iter().position(|&x| x == id))
            .flatten()
    }

    fn select_only(&mut self, kind: Kind, id: i64, ix: Option<usize>, cx: &mut Context<Self>) {
        self.marked.clear();
        self.anchor = ix;
        self.select(kind, id, cx);
        self.sync_marks(cx);
    }

    /// A click (or keyboard move, with no modifiers) on row `ix` of the
    /// shown list.
    fn click_row(&mut self, ix: usize, m: Modifiers, cx: &mut Context<Self>) {
        let Some(kind) = self.page.kind() else {
            return;
        };
        let ids = self.list_ids(kind, cx);
        let Some(&id) = ids.get(ix) else {
            return;
        };
        let same_kind = self.selected.is_some_and(|(k, _)| k == kind);
        if m.shift {
            let anchor = self.anchor.or(self.cursor(cx)).unwrap_or(ix);
            let (a, b) = (anchor.min(ix), anchor.max(ix));
            if !(m.secondary(cx) && same_kind) {
                self.marked.clear();
            }
            self.marked.extend(&ids[a..=b.min(ids.len() - 1)]);
            self.anchor = Some(anchor);
            self.select(kind, id, cx);
        } else if m.secondary(cx) {
            if !same_kind {
                self.marked.clear();
            }
            // Ctrl on a single selection starts a multiple one with it.
            if self.marked.is_empty()
                && let Some((_, cur)) = self.selected
                && same_kind
            {
                self.marked.insert(cur);
            }
            if self.marked.contains(&id) {
                self.marked.remove(&id);
                // The current row moves to another selected one.
                if self.selected == Some((kind, id))
                    && let Some(&next) = ids.iter().find(|x| self.marked.contains(x))
                {
                    self.select(kind, next, cx);
                }
            } else {
                self.marked.insert(id);
                self.select(kind, id, cx);
            }
            self.anchor = Some(ix);
        } else {
            self.select_only(kind, id, Some(ix), cx);
            return;
        }
        if self.marked.len() <= 1 {
            self.marked.clear();
        }
        self.sync_marks(cx);
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.page.kind() else {
            return;
        };
        let ids = self.list_ids(kind, cx);
        let Some(&first) = ids.first() else {
            return;
        };
        if !self
            .selected
            .is_some_and(|(k, id)| k == kind && ids.contains(&id))
        {
            self.select(kind, first, cx);
            self.anchor = Some(0);
        }
        self.marked = ids.into_iter().collect();
        if self.marked.len() <= 1 {
            self.marked.clear();
        }
        self.sync_marks(cx);
        cx.notify();
    }

    /// Esc: keep only the current row.
    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.marked.clear();
        self.anchor = self.cursor(cx);
        self.sync_marks(cx);
        cx.notify();
    }

    /// Move the current row by `delta` (clamped); with `extend`, grow the
    /// selection from the anchor as Shift+↑/↓ do.
    fn move_cursor(&mut self, to: MoveTo, extend: bool, cx: &mut Context<Self>) {
        let Some(kind) = self.page.kind() else {
            return;
        };
        let len = self.list_ids(kind, cx).len();
        if len == 0 {
            return;
        }
        let cur = self.cursor(cx);
        let ix = match (to, cur) {
            (MoveTo::First, _) => 0,
            (MoveTo::Last, _) => len - 1,
            (MoveTo::By(_), None) => 0,
            (MoveTo::By(d), Some(c)) => c.saturating_add_signed(d).min(len - 1),
        };
        let m = Modifiers {
            shift: extend,
            ..Modifiers::default()
        };
        if extend && self.anchor.is_none() {
            self.anchor = cur;
        }
        self.click_row(ix, m, cx);
        self.reveal_row(kind, ix, cx);
    }

    /// Scroll row `ix` into view and put the table's own cursor there.
    fn reveal_row(&mut self, kind: Kind, ix: usize, cx: &mut Context<Self>) {
        match kind {
            Kind::Album => self
                .album_scroll
                .scroll_to_item(ix, ScrollStrategy::Nearest),
            Kind::Item => {
                self.syncing_row = Some(ix);
                self.tracks.update(cx, |t, cx| t.set_selected_row(ix, cx));
            }
        }
    }

    /// Copy the selection to the track table for highlighting.
    fn sync_marks(&mut self, cx: &mut Context<Self>) {
        let marked = if self.selected.is_some_and(|(k, _)| k == Kind::Item) {
            self.marked.clone()
        } else {
            HashSet::new()
        };
        self.tracks.update(cx, |t, cx| {
            if t.delegate().marked != marked {
                t.delegate_mut().marked = marked;
                cx.notify();
            }
        });
    }

    /// Focus the shown list, for the keyboard.
    fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            Page::Albums => window.focus(&self.album_focus, cx),
            Page::Tracks => {
                let handle = self.tracks.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            _ => {}
        }
    }

    /// Enter: an album opens its tracks; a track plays.
    fn open_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some((Kind::Album, _)) => {
                let ids = self
                    .selected_ids(cx)
                    .map(|(_, ids)| ids)
                    .unwrap_or_default();
                if ids.is_empty() {
                    return;
                }
                let q = ids_query("album_id", &ids);
                self.search.update(cx, |s, cx| s.set_value(q, window, cx));
                self.show(Page::Tracks, window, cx);
                self.focus_list(window, cx);
            }
            Some((Kind::Item, _)) => self.play_selection(cx),
            None => {}
        }
    }

    fn request_detail(&mut self, kind: Kind, id: i64) {
        self.detail_gen += 1;
        self.backend
            .send(Req::Detail {
                generation: self.detail_gen,
                kind,
                id,
            })
            .ok();
    }

    fn on_response(&mut self, resp: Resp, window: &mut Window, cx: &mut Context<Self>) {
        match resp {
            Resp::Opened(result) => {
                self.opening = false;
                match result {
                    Ok(info) => {
                        self.info = Some(info);
                        self.open_error = None;
                    }
                    Err(e) => self.open_error = Some(e),
                }
                if let Some(start) = self.startup.take() {
                    self.apply_startup(start, window, cx);
                }
                // The first load happens once the library is open.
                if let Some(kind) = self.page.kind() {
                    let loaded = match kind {
                        Kind::Album => &self.albums_query,
                        Kind::Item => &self.tracks_query,
                    };
                    if loaded.is_none() {
                        let q = self.query_text(cx);
                        self.load(kind, q, cx);
                    }
                }
            }
            Resp::Rows {
                generation,
                kind,
                rows,
                counts,
            } => {
                if let (Some(info), Some((items, albums))) = (self.info.as_mut(), counts) {
                    info.items = items;
                    info.albums = albums;
                }
                if generation != self.list_gen {
                    return;
                }
                self.loading = false;
                match rows {
                    Ok(rows) => {
                        self.list_error = None;
                        match kind {
                            Kind::Album => self.albums = rows,
                            Kind::Item => self.tracks.update(cx, |t, cx| {
                                t.delegate_mut().set_rows(rows);
                                t.refresh(cx);
                            }),
                        }
                    }
                    Err(e) => self.list_error = Some(e),
                }
            }
            Resp::Detail { generation, detail } => {
                if generation != self.detail_gen {
                    return;
                }
                match detail {
                    Ok(d) => {
                        if std::mem::take(&mut self.play_when_loaded)
                            && let Some(path) = &d.path
                        {
                            cx.open_with_system(path);
                        }
                        self.detail = Some(d);
                        self.detail_error = None;
                        if let Some(start) = self.startup_after_detail.take() {
                            if start.edit {
                                self.toggle_edit(window, cx);
                            }
                            if let Some(action) = start.action {
                                self.open_action(action, true, window, cx);
                            }
                        }
                    }
                    Err(e) => {
                        self.detail = None;
                        self.detail_error = Some(e);
                    }
                }
            }
        }
    }

    fn apply_startup(&mut self, start: Startup, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = &start.plugin {
            self.plugin_selected = self.catalog.iter().find(|p| p.name == name).map(|p| p.name);
        }
        if start.shortcuts {
            self.show_shortcuts(window, cx);
        }
        if let Some(dark) = start.dark {
            let mode = if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            Theme::change(mode, Some(window), cx);
        }
        if let Some(q) = &start.query {
            self.search
                .update(cx, |s, cx| s.set_value(q.clone(), window, cx));
        }
        if let Some(page) = start.page {
            self.show(page, window, cx);
        }
        if let Some(cmd) = &start.run {
            self.run_typed_command(cmd, cx);
        }
        match start.select.clone() {
            Some((kind, ids)) => {
                self.select(kind, ids[0], cx);
                if ids.len() > 1 {
                    self.marked = ids.into_iter().collect();
                }
                self.sync_marks(cx);
                self.startup_after_detail = Some(start);
            }
            None => {
                if let Some(action) = start.action {
                    self.open_action(action, false, window, cx);
                }
            }
        }
    }

    // -- running beet ---------------------------------------------------

    fn start(&mut self, title: String, args: Vec<String>, mutates: bool, cx: &mut Context<Self>) {
        let id = self.runner.start(title, args, mutates);
        self.active_session = Some(id);
        self.follow_output = true;
        self.page = Page::Console;
        cx.notify();
    }

    /// Run a command without leaving the current page when it finishes
    /// quietly; switch to the console when it needs input or fails.
    fn start_background(
        &mut self,
        title: String,
        args: Vec<String>,
        mutates: bool,
        cx: &mut Context<Self>,
    ) {
        let id = self.runner.start(title, args, mutates);
        self.active_session = Some(id);
        self.follow_output = true;
        cx.notify();
    }

    fn on_session_finished(&mut self, id: usize, cx: &mut Context<Self>) {
        let s = &self.runner.sessions[id];
        let (mutates, ok) = (s.mutates, s.succeeded());
        if mutates {
            self.reload(cx);
        }
        // A background run that failed: show it.
        if !ok && self.page != Page::Console {
            self.active_session = Some(id);
            self.page = Page::Console;
        }
    }

    fn answer(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(s) = self
            .active_session
            .and_then(|i| self.runner.sessions.get_mut(i))
            && s.running()
        {
            s.send(text);
            self.follow_output = true;
            self.console_scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn run_typed_command(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let mut args = split_words(text);
        if args.first().is_some_and(|a| a == "beet") {
            args.remove(0);
        }
        if args.is_empty() {
            return false;
        }
        let read_only = matches!(
            args[0].as_str(),
            "ls" | "list" | "stats" | "fields" | "version" | "help" | "config"
        );
        self.start(runner::shell_join(&args), args, !read_only, cx);
        true
    }

    /// The selected ids (several, or the current row), in list order.
    fn selected_ids(&self, cx: &App) -> Option<(Kind, Vec<i64>)> {
        let (kind, id) = self.selected?;
        if self.marked.len() <= 1 {
            return Some((kind, vec![id]));
        }
        let mut ids: Vec<i64> = self
            .list_ids(kind, cx)
            .into_iter()
            .filter(|x| self.marked.contains(x))
            .collect();
        // Selected rows the list no longer shows (after a search).
        if ids.len() < self.marked.len() {
            let mut rest: Vec<i64> = self
                .marked
                .iter()
                .copied()
                .filter(|x| !ids.contains(x))
                .collect();
            rest.sort_unstable();
            ids.extend(rest);
        }
        Some((kind, ids))
    }

    /// Whether the selection is the whole shown list, whose search query
    /// then stands for it.
    fn selection_is_whole_list(&self, cx: &App) -> bool {
        let Some((kind, _)) = self.selected else {
            return false;
        };
        let ids = self.list_ids(kind, cx);
        self.marked.len() > 1
            && self.marked.len() == ids.len()
            && ids.iter().all(|x| self.marked.contains(x))
    }

    /// The query for the selection in its own kind (with `-a` for albums).
    fn selection_target(&self, cx: &App) -> Option<(Kind, Vec<String>)> {
        let (kind, ids) = self.selected_ids(cx)?;
        if self.selection_is_whole_list(cx) {
            return Some((kind, split_words(&self.query_text(cx))));
        }
        Some((kind, vec![ids_query("id", &ids)]))
    }

    /// The query that matches the selection (albums or tracks) as items,
    /// for plugin commands, which mostly take item queries.
    fn selection_query_cx(&self, cx: &App) -> Option<(Kind, Vec<String>)> {
        let (kind, ids) = self.selected_ids(cx)?;
        Some(match kind {
            Kind::Album => (kind, vec![ids_query("album_id", &ids)]),
            Kind::Item if self.selection_is_whole_list(cx) => {
                (kind, split_words(&self.query_text(cx)))
            }
            Kind::Item => (kind, vec![ids_query("id", &ids)]),
        })
    }

    /// Run `args` + the selection's item query in the background.
    fn run_quick(&mut self, args: &[String], cx: &mut Context<Self>) {
        if let Some((_, q)) = self.selection_query_cx(cx) {
            if q.iter().map(String::len).sum::<usize>() > MAX_QUERY_ARG {
                return;
            }
            let mut a = args.to_vec();
            a.extend(q);
            self.start_background(runner::shell_join_short(&a), a, true, cx);
        }
    }

    fn run_on_selection(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let mut args = split_words(text);
        if args.first().is_some_and(|a| a == "beet") {
            args.remove(0);
        }
        let Some((_, q)) = self.selection_query_cx(cx) else {
            return false;
        };
        if args.is_empty() {
            return false;
        }
        args.extend(q);
        self.start(runner::shell_join_short(&args), args, true, cx);
        true
    }

    fn has_plugin(&self, name: &str) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| i.plugins.iter().any(|p| p == name))
    }

    /// Play the selection: with the `play` plugin through `beet play` (its
    /// configured player, albums too), otherwise the file in the system's
    /// default player.
    fn play_selection(&mut self, cx: &mut Context<Self>) {
        if self.has_plugin("play") {
            if let Some((_, q)) = self.selection_query_cx(cx) {
                let mut args = vec!["play".to_string(), "-y".into()];
                args.extend(q);
                self.start_background(runner::shell_join_short(&args), args, false, cx);
            }
        } else {
            match &self.detail {
                Some(d) if Some((d.kind, d.id)) == self.selected => {
                    if let Some(path) = &d.path {
                        cx.open_with_system(path);
                    }
                }
                // Just selected (a double click): its detail is on the way.
                _ => self.play_when_loaded = true,
            }
        }
    }

    /// The `replace` plugin: pick a new audio file for the selected track;
    /// `beet replace` asks to confirm in the console.
    fn replace_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.len() > 1 {
            return;
        }
        let Some((Kind::Item, q)) = self.selection_query_cx(cx) else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Replace with".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(file) = paths.into_iter().next()
            {
                let mut args = vec!["replace".to_string()];
                args.extend(q);
                args.push(file.to_string_lossy().into_owned());
                this.update(cx, |this, cx| {
                    this.start(runner::shell_join(&args), args, true, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        let target = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        cx.open_with_system(&target);
    }

    // -- dialogs --------------------------------------------------------

    fn open_action(
        &mut self,
        action: Action,
        on_selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (kind, query, description, count) = if on_selection && self.marked.len() > 1 {
            let Some((kind, query)) = self.selection_target(cx) else {
                return;
            };
            let n = self.marked.len();
            (kind, query, "the selection".to_string(), Some(n))
        } else if on_selection {
            let Some((kind, id)) = self.selected else {
                return;
            };
            let name = self
                .detail
                .as_ref()
                .map(|d| d.title.clone())
                .unwrap_or_default();
            let what = if kind == Kind::Album {
                "album"
            } else {
                "track"
            };
            (
                kind,
                vec![format!("id:{id}")],
                format!("the {what} “{name}”"),
                None,
            )
        } else {
            let Some(kind) = self.page.kind() else {
                return;
            };
            let q = self.query_text(cx);
            let count = match kind {
                Kind::Album => self.albums.len(),
                Kind::Item => self.tracks.read(cx).delegate().rows.len(),
            };
            let description = if q.is_empty() {
                "everything".to_string()
            } else {
                format!("the query “{q}”")
            };
            (kind, split_words(&q), description, Some(count))
        };
        let form = cx.new(|cx| ActionForm {
            action,
            kind,
            query,
            description,
            count,
            assignments: cx.new(|cx| {
                InputState::new(window, cx).placeholder("genre=Jazz comments! \"title=A b\"")
            }),
            dest: cx.new(|cx| InputState::new(window, cx).placeholder("~/Music/export")),
            delete: false,
            copy: false,
            export: false,
            pretend: false,
            no_move: false,
            no_write: false,
            force: false,
        });
        let app = cx.entity().downgrade();
        let title = format!(
            "{} {}",
            action.title(),
            if kind == Kind::Album {
                "(albums)"
            } else {
                "(tracks)"
            }
        );
        window.open_dialog(cx, move |dialog, _, cx| {
            let form_view_entity = form.clone();
            let form = form.clone();
            let app = app.clone();
            let danger = action == Action::Remove;
            let run = Button::new("run")
                .label(action.title())
                .map(|b| if danger { b.danger() } else { b.primary() })
                .on_click(move |_, window, cx| {
                    let args = form.read(cx).args(cx);
                    if args.len() <= 3 && form.read(cx).action == Action::Modify {
                        // Nothing to assign.
                        return;
                    }
                    let title = runner::shell_join_short(&args);
                    app.update(cx, |this, cx| this.start(title, args, true, cx))
                        .ok();
                    window.close_dialog(cx);
                });
            dialog
                .title(title.clone())
                .w(px(560.))
                .child(form_view(&form_view_entity, cx))
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(run),
                )
        });
    }

    fn browse_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Import".into()),
        });
        let input = self.import.paths.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let text = runner::shell_join(
                    &paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                );
                input
                    .update_in(cx, |s, window, cx| s.set_value(text, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    // -- configuration --------------------------------------------------

    fn load_config_file(&mut self, window: &mut Window, cx: &mut Context<Self>, force: bool) {
        let Some(path) = self.info.as_ref().map(|i| i.user_config.clone()) else {
            return;
        };
        if !force && self.config_path.as_ref() == Some(&path) {
            return;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.config_saved = text.clone();
        self.config_path = Some(path);
        self.config_editor
            .update(cx, |s, cx| s.set_value(text, window, cx));
        self.config_message = None;
    }

    fn save_config(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let text = self.config_editor.read(cx).value().to_string();
        if let Err(e) = serde_yaml_check(&text) {
            self.config_message = Some((false, format!("Not saved: invalid YAML: {e}")));
            cx.notify();
            return;
        }
        let result = path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&path, &text));
        match result {
            Ok(()) => {
                self.config_saved = text;
                self.config_message = Some((true, format!("Saved {}", path.display())));
                self.reload(cx);
            }
            Err(e) => self.config_message = Some((false, format!("Not saved: {e}"))),
        }
        cx.notify();
    }
}

/// A rough YAML sanity check before saving (the library reports the real
/// errors when it reloads).
fn serde_yaml_check(text: &str) -> Result<(), String> {
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('\t') {
            return Err(format!(
                "line {}: tabs are not allowed for indentation",
                n + 1
            ));
        }
    }
    Ok(())
}

/// Nothing worth showing: empty, or a zero ("0", "00", "0.0", "0:00").
fn is_blank(value: &str) -> bool {
    value.chars().all(|c| matches!(c, '0' | '.' | ':' | ' '))
}

fn form_view(form: &Entity<ActionForm>, _: &App) -> AnyElement {
    form.clone().into_any_element()
}

impl Row {
    fn album_id(&self) -> i64 {
        self.count
    }
}

// ---------------------------------------------------------------------------
// Rendering.

fn ansi_color(c: AnsiColor, cx: &App) -> Hsla {
    let t = cx.theme();
    match c {
        AnsiColor::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into(),
        AnsiColor::Indexed(i) => match i {
            0 | 8 => t.muted_foreground,
            1 => t.red,
            2 => t.green,
            3 => t.yellow,
            4 => t.blue,
            5 => t.magenta,
            6 => t.cyan,
            7 | 15 => t.foreground,
            9 => t.red_light,
            10 => t.green_light,
            11 => t.yellow_light,
            12 => t.blue_light,
            13 => t.magenta_light,
            14 => t.cyan_light,
            _ => t.foreground,
        },
    }
}

fn styled_line(line: &runner::Line, cx: &App) -> StyledText {
    let highlights: Vec<(Range<usize>, HighlightStyle)> = line
        .styles
        .iter()
        .map(|(range, style)| {
            (
                range.clone(),
                HighlightStyle {
                    color: style.color.map(|c| ansi_color(c, cx)),
                    font_weight: style.bold.then_some(FontWeight::BOLD),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect();
    StyledText::new(SharedString::from(line.text.clone())).with_highlights(highlights)
}

fn section_title(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.into().to_uppercase())
}

fn cover(path: Option<&PathBuf>, size: Pixels, cx: &App) -> AnyElement {
    let frame = div()
        .size(size)
        .flex_none()
        .rounded_md()
        .overflow_hidden()
        .bg(cx.theme().muted)
        .border_1()
        .border_color(cx.theme().border);
    match path {
        Some(p) => frame
            .child(img(p.clone()).size(size).object_fit(ObjectFit::Cover))
            .into_any_element(),
        None => frame
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child("♪")
            .into_any_element(),
    }
}

impl BeetsApp {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let running = self.runner.sessions.iter().filter(|s| s.running()).count();
        let waiting = self
            .runner
            .sessions
            .iter()
            .filter(|s| s.prompt().is_some())
            .count();
        let nav = Page::ALL.iter().map(|&page| {
            let selected = self.page == page;
            let badge = match page {
                Page::Albums => self.info.as_ref().map(|i| i.albums.to_string()),
                Page::Tracks => self.info.as_ref().map(|i| i.items.to_string()),
                Page::Plugins => self
                    .info
                    .as_ref()
                    .map(|i| format!("{} on", i.enabled.len())),
                Page::Console if waiting > 0 => Some(format!("{waiting} waiting")),
                Page::Console if running > 0 => Some(format!("{running} running")),
                _ => None,
            };
            h_flex()
                .id(SharedString::from(format!("nav-{}", page.label())))
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |d| {
                    d.bg(theme.sidebar_accent)
                        .text_color(theme.sidebar_accent_foreground)
                })
                .hover(|d| d.bg(theme.sidebar_accent))
                .child(Icon::new(page.icon()).small())
                .child(div().flex_1().child(page.label()))
                .children(badge.map(|b| {
                    div()
                        .text_xs()
                        .text_color(if page == Page::Console && waiting > 0 {
                            theme.warning
                        } else {
                            theme.muted_foreground
                        })
                        .child(b)
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show(page, window, cx);
                    this.focus_list(window, cx);
                }))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "{}  (Ctrl+{})",
                        page.label(),
                        Page::ALL.iter().position(|&p| p == page).unwrap_or(0) + 1
                    ))
                    .build(window, cx)
                })
        });
        let lib = match (&self.info, &self.open_error) {
            (_, Some(e)) => div()
                .text_xs()
                .text_color(theme.danger)
                .child(format!("Library error: {e}"))
                .into_any_element(),
            (Some(info), None) => v_flex()
                .gap_0p5()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(format!("{}", info.directory.display())),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(format!("db: {}", info.library.display())),
                )
                .child(format!(
                    "{} plugin{}",
                    info.plugins.len(),
                    if info.plugins.len() == 1 { "" } else { "s" }
                ))
                .into_any_element(),
            (None, None) => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Opening the library…")
                .into_any_element(),
        };
        let dark = theme.is_dark();
        v_flex()
            .w(px(210.))
            .flex_none()
            .h_full()
            .p_2()
            .gap_1()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child("beets"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("rs"),
                    ),
            )
            .children(nav)
            .child(div().flex_1())
            .child(lib)
            .child(
                h_flex()
                    .gap_1()
                    .pt_2()
                    .child(
                        Button::new("reload")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .small()
                            .tooltip("Reload the library (F5)")
                            .loading(self.opening)
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        Button::new("theme")
                            .label(if dark { "Light" } else { "Dark" })
                            .ghost()
                            .small()
                            .on_click(move |_, window, cx| {
                                Theme::change(
                                    if dark {
                                        ThemeMode::Light
                                    } else {
                                        ThemeMode::Dark
                                    },
                                    Some(window),
                                    cx,
                                );
                                window.refresh();
                            }),
                    ),
            )
    }

    fn render_search_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.page.kind();
        let count = match kind {
            Some(Kind::Album) => self.albums.len(),
            Some(Kind::Item) => self.tracks.read(cx).delegate().rows.len(),
            None => 0,
        };
        let what = if kind == Some(Kind::Album) {
            "albums"
        } else {
            "tracks"
        };
        let actions = [
            ("q-modify", "Modify…", Action::Modify),
            ("q-update", "Update", Action::Update),
            ("q-write", "Write", Action::Write),
            ("q-move", "Move…", Action::Move),
            ("q-remove", "Remove…", Action::Remove),
        ];
        v_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.search)
                                .prefix(Icon::new(IconName::Search).small())
                                .cleanable(true),
                        ),
                    )
                    .child(
                        div()
                            .w(px(150.))
                            .text_sm()
                            .text_right()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.loading {
                                "searching…".to_string()
                            } else if self.marked.len() > 1
                                && self.selected.is_some_and(|(k, _)| Some(k) == kind)
                            {
                                format!("{} of {count} selected", self.marked.len())
                            } else {
                                format!("{count} {what}")
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .mr_1()
                            .child("All matching:"),
                    )
                    .children(actions.into_iter().map(|(id, label, action)| {
                        Button::new(id)
                            .label(label)
                            .xsmall()
                            .ghost()
                            .when(action == Action::Remove, |b| b.danger())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_action(action, false, window, cx)
                            }))
                    })),
            )
            .children(
                self.list_error
                    .clone()
                    .map(|e| div().text_sm().text_color(cx.theme().danger).child(e)),
            )
    }

    fn render_albums(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let marked = self.marked.clone();
        let count = self.albums.len();
        let list = uniform_list(
            "albums",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let theme = cx.theme();
                range
                    .filter_map(|ix| {
                        let row = this.albums.get(ix)?;
                        let id = row.id;
                        let is_current = selected == Some((Kind::Album, id));
                        let is_selected = is_current
                            || (selected.is_some_and(|(k, _)| k == Kind::Album)
                                && marked.contains(&id));
                        Some(
                            h_flex()
                                .id(("album", ix))
                                .w_full()
                                .h(px(60.))
                                .px_3()
                                .gap_3()
                                .cursor_pointer()
                                .border_b_1()
                                .border_color(theme.border)
                                .when(is_selected, |d| {
                                    d.bg(if marked.len() > 1 {
                                        selection_bg(cx)
                                    } else {
                                        theme.list_active
                                    })
                                })
                                .when(is_current && marked.len() > 1, |d| {
                                    d.border_l_2().border_color(theme.primary)
                                })
                                .hover(|d| d.bg(theme.list_hover))
                                .child(cover(row.art.as_ref(), px(44.), cx))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(if row.title.is_empty() {
                                                    "(untitled)".to_string()
                                                } else {
                                                    row.title.clone()
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(row.artist.clone()),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .items_end()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(row.year.clone())
                                        .child(format!(
                                            "{} track{}",
                                            row.count,
                                            if row.count == 1 { "" } else { "s" }
                                        )),
                                )
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    window.focus(&this.album_focus, cx);
                                    this.click_row(ix, e.modifiers(), cx);
                                    if e.click_count() == 2 && !e.modifiers().shift {
                                        this.open_selection(window, cx);
                                    }
                                })),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.album_scroll)
        .flex_1()
        .h_full();
        v_flex()
            .size_full()
            .track_focus(&self.album_focus)
            .when(count == 0 && !self.loading, |d| {
                d.child(empty_state(
                    if self.query_text(cx).is_empty() {
                        "The library has no albums yet. Import some music from the Import page."
                    } else {
                        "No albums match the query."
                    },
                    cx,
                ))
            })
            .child(list)
    }

    fn render_tracks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.tracks.read(cx).delegate().rows.is_empty();
        v_flex()
            .size_full()
            .when(empty && !self.loading, |d| {
                d.child(empty_state(
                    if self.query_text(cx).is_empty() {
                        "The library has no tracks yet. Import some music from the Import page."
                    } else {
                        "No tracks match the query."
                    },
                    cx,
                ))
            })
            .child(
                DataTable::new(&self.tracks)
                    .stripe(true)
                    .bordered(false)
                    .scrollbar_visible(true, true),
            )
    }

    fn render_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.marked.len() > 1 {
            return self.render_bulk(cx);
        }
        let theme = cx.theme().clone();
        let Some(detail) = self.detail.clone() else {
            let msg = match (&self.detail_error, self.selected) {
                (Some(e), _) => e.clone(),
                (None, Some(_)) => "Loading…".into(),
                (None, None) => "Select an album or a track to see its details.".into(),
            };
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_6()
                .text_color(theme.muted_foreground)
                .child(msg)
                .into_any_element();
        };
        let is_album = detail.kind == Kind::Album;
        let editing = self.edit.is_some();
        let path = detail.path.clone();

        let header =
            h_flex()
                .gap_4()
                .items_start()
                .child(cover(detail.art.as_ref(), px(132.), cx))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        .overflow_hidden()
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            if is_album {
                                format!("ALBUM · id {}", detail.id)
                            } else {
                                format!("TRACK · id {}", detail.id)
                            },
                        ))
                        .child(div().text_xl().font_weight(FontWeight::BOLD).child(
                            if detail.title.is_empty() {
                                "(untitled)".to_string()
                            } else {
                                detail.title.clone()
                            },
                        ))
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(detail.subtitle.clone()),
                        )
                        .children(path.clone().map(|p| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(p.display().to_string())
                        })),
                );

        let toolbar = h_flex()
            .flex_wrap()
            .gap_1()
            .child(
                Button::new("edit")
                    .label(if editing { "Cancel edit" } else { "Edit" })
                    .small()
                    .tooltip(if editing { "Esc" } else { "F2" })
                    .when(!editing, |b| b.primary())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_edit(window, cx))),
            )
            .when(!is_album || self.has_plugin("play"), |d| {
                d.child(
                    Button::new("play")
                        .label("Play")
                        .small()
                        .tooltip("Space")
                        .on_click(cx.listener(|this, _, _, cx| this.play_selection(cx))),
                )
            })
            .when(!is_album && self.has_plugin("replace"), |d| {
                d.child(
                    Button::new("replace")
                        .label("Replace file…")
                        .small()
                        .tooltip("Swap in a new audio file for this track (replace plugin)")
                        .on_click(cx.listener(|this, _, window, cx| this.replace_file(window, cx))),
                )
            })
            .children(path.clone().map(|p| {
                Button::new("reveal")
                    .label("Show folder")
                    .small()
                    .tooltip("Ctrl+O")
                    .on_click(cx.listener(move |this, _, _, cx| this.reveal(&p, cx)))
            }))
            .child(
                Button::new("sel-update")
                    .label("Update")
                    .small()
                    .tooltip("Re-read the tags from the files (Ctrl+U)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_action(Action::Update, true, window, cx)
                    })),
            )
            .child(
                Button::new("sel-write")
                    .label("Write tags")
                    .small()
                    .tooltip("Ctrl+T")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_action(Action::Write, true, window, cx)
                    })),
            )
            .child(
                Button::new("sel-move")
                    .label("Move…")
                    .small()
                    .tooltip("Ctrl+M")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_action(Action::Move, true, window, cx)
                    })),
            )
            .child(
                Button::new("sel-remove")
                    .label("Remove…")
                    .small()
                    .tooltip("Delete")
                    .danger()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_action(Action::Remove, true, window, cx)
                    })),
            );

        let plugin_row = self.render_plugin_row(cx);

        let body = if let Some(form) = &self.edit {
            self.render_edit_form(form, window, cx)
        } else {
            self.render_fields(&detail, cx)
        };

        let tracks = (is_album && !detail.tracks.is_empty() && !editing).then(|| {
            v_flex()
                .gap_1()
                .child(section_title(format!("{} tracks", detail.tracks.len()), cx))
                .children(detail.tracks.iter().map(|t| {
                    let id = t.id;
                    h_flex()
                        .id(("detail-track", id as usize))
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|d| d.bg(theme.list_hover))
                        .child(
                            div()
                                .w(px(40.))
                                .text_color(theme.muted_foreground)
                                .child(t.track.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(t.title.clone()),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(t.length.clone()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.show(Page::Tracks, window, cx);
                            this.select(Kind::Item, id, cx);
                        }))
                }))
        });

        v_flex()
            .id("detail")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(header)
            .child(toolbar)
            .child(plugin_row)
            .children(tracks)
            .child(body)
            .into_any_element()
    }

    fn quick_commands(&self) -> Vec<(&'static str, &'static str, Vec<String>)> {
        let plugins = self
            .info
            .as_ref()
            .map(|i| i.plugins.clone())
            .unwrap_or_default();
        [
            (
                "fetchart",
                "Fetch art",
                vec!["fetchart".to_string(), "-f".into()],
            ),
            (
                "embedart",
                "Embed art",
                vec!["embedart".into(), "-y".into()],
            ),
            ("lyrics", "Fetch lyrics", vec!["lyrics".into(), "-f".into()]),
            ("lastgenre", "Last.fm genre", vec!["lastgenre".into()]),
            ("replaygain", "ReplayGain", vec!["replaygain".into()]),
            ("chroma", "Fingerprint", vec!["fingerprint".into()]),
            ("mbsync", "MusicBrainz sync", vec!["mbsync".into()]),
            ("scrub", "Scrub tags", vec!["scrub".into()]),
            ("badfiles", "Check files", vec!["bad".into()]),
            ("autobpm", "Detect BPM", vec!["autobpm".into()]),
            ("keyfinder", "Find key", vec!["keyfinder".into()]),
            (
                "acousticbrainz",
                "AcousticBrainz",
                vec!["acousticbrainz".into()],
            ),
            ("parentwork", "Parent work", vec!["parentwork".into()]),
            ("ftintitle", "Feat. to title", vec!["ftintitle".into()]),
            ("titlecase", "Title case", vec!["titlecase".into()]),
            ("zero", "Zero fields", vec!["zero".into()]),
            ("thumbnails", "Thumbnails", vec!["thumbnails".into()]),
            ("spotify", "Spotify sync", vec!["spotifysync".into()]),
            ("deezer", "Deezer sync", vec!["deezerupdate".into()]),
            ("tidal", "Tidal sync", vec!["tidalsync".into()]),
            ("bpsync", "Beatport sync", vec!["bpsync".into()]),
        ]
        .into_iter()
        .filter(|(p, _, _)| plugins.iter().any(|x| x == p))
        .collect()
    }

    fn render_plugin_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let quick = self.quick_commands();
        v_flex()
            .gap_1()
            .child(section_title("Run on this selection", cx))
            .when(!quick.is_empty(), |d| {
                d.child(h_flex().flex_wrap().gap_1().children(quick.into_iter().map(
                    |(id, label, args)| {
                        Button::new(SharedString::from(format!("quick-{id}")))
                            .label(label)
                            .xsmall()
                            .outline()
                            .on_click(cx.listener(move |this, _, _, cx| this.run_quick(&args, cx)))
                    },
                )))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.plugin_cmd).small()))
                    .child(
                        Button::new("browse-plugins")
                            .label("More plugins…")
                            .xsmall()
                            .ghost()
                            .tooltip("Browse and enable plugins (Ctrl+4)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show(Page::Plugins, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_fields(&self, detail: &Detail, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let filter = self.field_filter.read(cx).value().to_lowercase();
        let rows = detail
            .fields
            .iter()
            .filter(|f| {
                filter.is_empty()
                    || f.key.to_lowercase().contains(&filter)
                    || f.display.to_lowercase().contains(&filter)
            })
            .filter(|f| !filter.is_empty() || !is_blank(&f.display))
            .map(|f| {
                h_flex()
                    .gap_3()
                    .py_0p5()
                    .items_start()
                    .child(
                        div()
                            .w(px(170.))
                            .flex_none()
                            .text_sm()
                            .text_color(if f.flex {
                                theme.info
                            } else {
                                theme.muted_foreground
                            })
                            .child(f.key.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("field-{}", f.key)))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(f.display.clone())
                            .when(f.display.len() > 40, |d| {
                                let full = f.display.clone();
                                d.tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(full.clone())
                                        .build(window, cx)
                                })
                            }),
                    )
            });
        v_flex()
            .gap_1()
            .child(
                h_flex().gap_2().child(section_title("Fields", cx)).child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("(empty ones hidden; blue = flexible attribute)"),
                ),
            )
            .child(Input::new(&self.field_filter).small().cleanable(true))
            .children(rows)
            .into_any_element()
    }

    fn toggle_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            self.edit = None;
            cx.notify();
            return;
        }
        let Some(detail) = &self.detail else {
            return;
        };
        let fields = detail
            .fields
            .iter()
            .filter(|f| f.edit.is_some())
            .map(|f| {
                let value = f.edit.clone().unwrap_or_default();
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                (f.clone(), input)
            })
            .collect();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter fields"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        self.edit = Some(EditForm {
            kind: detail.kind,
            id: detail.id,
            fields,
            filter,
            new_key: cx.new(|cx| InputState::new(window, cx).placeholder("new field")),
            new_value: cx.new(|cx| InputState::new(window, cx).placeholder("value")),
            move_files: true,
            write_tags: true,
        });
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.edit else {
            return;
        };
        let changes = form.changes(cx);
        if changes.is_empty() {
            self.edit = None;
            cx.notify();
            return;
        }
        let mut args = vec!["modify".to_string(), "-y".into()];
        if form.kind == Kind::Album {
            args.push("-a".into());
        }
        if !form.move_files {
            args.push("-M".into());
        }
        if !form.write_tags {
            args.push("-W".into());
        }
        args.push(format!("id:{}", form.id));
        args.extend(changes);
        self.edit = None;
        self.start_background(runner::shell_join(&args), args, true, cx);
    }

    fn render_edit_form(
        &self,
        form: &EditForm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let filter = form.filter.read(cx).value().to_lowercase();
        let changed = form.changes(cx);
        let rows = form
            .fields
            .iter()
            .filter(|(f, _)| filter.is_empty() || f.key.to_lowercase().contains(&filter))
            .map(|(f, input)| {
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .w(px(170.))
                            .flex_none()
                            .text_sm()
                            .text_color(if f.flex {
                                theme.info
                            } else {
                                theme.muted_foreground
                            })
                            .child(f.key.clone()),
                    )
                    .child(div().flex_1().child(Input::new(input).small()))
            });
        let (move_files, write_tags) = (form.move_files, form.write_tags);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(section_title("Edit fields", cx))
                    .child(div().flex_1())
                    .child(
                        Checkbox::new("edit-move")
                            .label("Move files")
                            .checked(move_files)
                            .on_click(cx.listener(|this, v: &bool, _, cx| {
                                if let Some(f) = &mut this.edit {
                                    f.move_files = *v;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("edit-write")
                            .label("Write tags")
                            .checked(write_tags)
                            .on_click(cx.listener(|this, v: &bool, _, cx| {
                                if let Some(f) = &mut this.edit {
                                    f.write_tags = *v;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-edit")
                            .label(if changed.is_empty() {
                                "No changes".to_string()
                            } else {
                                format!(
                                    "Save {} change{}",
                                    changed.len(),
                                    if changed.len() == 1 { "" } else { "s" }
                                )
                            })
                            .primary()
                            .small()
                            .disabled(changed.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                    ),
            )
            .when(form.kind == Kind::Album, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Album fields also change on the album's tracks."),
                )
            })
            .child(Input::new(&form.filter).small().cleanable(true))
            .children(rows)
            .child(
                h_flex()
                    .gap_3()
                    .pt_2()
                    .child(
                        div()
                            .w(px(170.))
                            .flex_none()
                            .child(Input::new(&form.new_key).small()),
                    )
                    .child(div().flex_1().child(Input::new(&form.new_value).small())),
            )
            .when(!changed.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(format!("beet modify … {}", runner::shell_join(&changed))),
                )
            })
            .key_context("EditForm")
            .on_action(cx.listener(|this, _: &SaveEdit, window, cx| {
                this.save_edit(cx);
                this.focus_list(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CancelEdit, window, cx| {
                this.edit = None;
                this.focus_list(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Put the keyboard in the edit form's first field.
    fn focus_edit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, input)) = self.edit.as_ref().and_then(|f| f.fields.first()) {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    /// The detail pane for several selected albums or tracks.
    fn render_bulk(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some((kind, ids)) = self.selected_ids(cx) else {
            return div().into_any_element();
        };
        let is_album = kind == Kind::Album;
        let what = if is_album { "albums" } else { "tracks" };
        let rows: std::collections::HashMap<i64, Row> = match kind {
            Kind::Album => self.albums.iter().map(|r| (r.id, r.clone())).collect(),
            Kind::Item => self
                .tracks
                .read(cx)
                .delegate()
                .rows
                .iter()
                .map(|r| (r.id, r.clone()))
                .collect(),
        };
        let summary = if is_album {
            let tracks: i64 = ids
                .iter()
                .filter_map(|id| rows.get(id))
                .map(|r| r.count)
                .sum();
            format!("{tracks} tracks in all")
        } else {
            let secs: i64 = ids
                .iter()
                .filter_map(|id| rows.get(id))
                .map(|r| duration_secs(&r.length))
                .sum();
            format!(
                "{}:{:02}:{:02} in all",
                secs / 3600,
                secs / 60 % 60,
                secs % 60
            )
        };
        let plugin_row = self.render_plugin_row(cx);
        let too_long = self
            .selection_target(cx)
            .is_some_and(|(_, q)| q.iter().map(String::len).sum::<usize>() > MAX_QUERY_ARG);

        let button = |id: &'static str, label: &'static str, keys: &'static str, action: Action| {
            Button::new(id)
                .label(label)
                .small()
                .tooltip(keys)
                .map(|b| match action {
                    Action::Remove => b.danger(),
                    Action::Modify => b.primary(),
                    _ => b,
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_action(action, true, window, cx)
                }))
        };
        let toolbar = h_flex()
            .flex_wrap()
            .gap_1()
            .child(button(
                "bulk-modify",
                "Modify all…",
                "Set fields on every selected one (F2)",
                Action::Modify,
            ))
            .when(self.has_plugin("play"), |d| {
                d.child(
                    Button::new("bulk-play")
                        .label("Play")
                        .small()
                        .tooltip("Space")
                        .on_click(cx.listener(|this, _, _, cx| this.play_selection(cx))),
                )
            })
            .child(button(
                "bulk-update",
                "Update",
                "Re-read the tags from the files (Ctrl+U)",
                Action::Update,
            ))
            .child(button("bulk-write", "Write tags", "Ctrl+T", Action::Write))
            .child(button("bulk-move", "Move…", "Ctrl+M", Action::Move))
            .child(button("bulk-remove", "Remove…", "Delete", Action::Remove))
            .child(
                Button::new("bulk-none")
                    .label("Select one")
                    .small()
                    .ghost()
                    .tooltip("Keep only the current row (Esc)")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx))),
            );

        const SHOWN: usize = 300;
        let current = self.selected.map(|(_, id)| id);
        let list = ids
            .iter()
            .take(SHOWN)
            .filter_map(|id| rows.get(id))
            .map(|r| {
                let id = r.id;
                h_flex()
                    .id(("bulk-row", id as usize))
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(current == Some(id), |d| d.bg(theme.list_active))
                    .hover(|d| d.bg(theme.list_hover))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(if r.title.is_empty() {
                                "(untitled)".to_string()
                            } else {
                                r.title.clone()
                            }),
                    )
                    .child(
                        div()
                            .max_w(px(180.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(r.artist.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let ix = this.list_ids(kind, cx).iter().position(|&x| x == id);
                        this.select_only(kind, id, ix, cx);
                        if let Some(ix) = ix {
                            this.reveal_row(kind, ix, cx);
                        }
                    }))
            });

        v_flex()
            .id("bulk")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child("SELECTION"))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{} {what} selected", ids.len())),
                    )
                    .child(div().text_color(theme.muted_foreground).child(summary))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Ctrl+Click adds or removes · Shift+Click selects a range · Ctrl+A selects all · Esc keeps one"),
                    ),
            )
            .when(too_long, |d| {
                d.child(div().text_sm().text_color(theme.danger).child(
                    "Too many selected to pass as ids: select all with Ctrl+A to run on the search instead.",
                ))
            })
            .child(toolbar)
            .child(plugin_row)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(section_title(
                        if ids.len() > SHOWN {
                            format!("First {SHOWN} of {}", ids.len())
                        } else {
                            "Selected".to_string()
                        },
                        cx,
                    ))
                    .children(list),
            )
            .into_any_element()
    }

    fn show_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme().clone();
            let mut body = v_flex().gap_1();
            let mut last = "";
            for &(keys, what, area) in SHORTCUTS {
                if area != last {
                    body = body.child(
                        div()
                            .pt_2()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(area.to_uppercase()),
                    );
                    last = area;
                }
                body = body.child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .w(px(240.))
                                .flex_none()
                                .text_sm()
                                .whitespace_nowrap()
                                .font_family(theme.mono_font_family.clone())
                                .child(keys),
                        )
                        .child(div().flex_1().text_sm().child(what)),
                );
            }
            dialog.title("Keyboard shortcuts").w(px(700.)).child(body)
        });
    }

    fn render_library(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = match self.page {
            Page::Albums => self.render_albums(cx).into_any_element(),
            _ => self.render_tracks(cx).into_any_element(),
        };
        h_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w(px(380.))
                    .child(self.render_search_bar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .key_context("Library")
                            .on_action(
                                cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)),
                            )
                            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                                this.clear_selection(cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorUp, _, cx| {
                                this.move_cursor(MoveTo::By(-1), false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorDown, _, cx| {
                                this.move_cursor(MoveTo::By(1), false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorFirst, _, cx| {
                                this.move_cursor(MoveTo::First, false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorLast, _, cx| {
                                this.move_cursor(MoveTo::Last, false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorPageUp, _, cx| {
                                this.move_cursor(MoveTo::By(-10), false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &CursorPageDown, _, cx| {
                                this.move_cursor(MoveTo::By(10), false, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendUp, _, cx| {
                                this.move_cursor(MoveTo::By(-1), true, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendDown, _, cx| {
                                this.move_cursor(MoveTo::By(1), true, cx)
                            }))
                            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                                this.open_selection(window, cx)
                            }))
                            .on_action(
                                cx.listener(|this, _: &PlaySelection, _, cx| {
                                    this.play_selection(cx)
                                }),
                            )
                            .on_action(cx.listener(|this, _: &EditSelection, window, cx| {
                                if this.marked.len() > 1 {
                                    this.open_action(Action::Modify, true, window, cx);
                                } else if this.edit.is_none() {
                                    this.toggle_edit(window, cx);
                                    this.focus_edit_form(window, cx);
                                }
                            }))
                            .on_action(cx.listener(|this, _: &RemoveSelection, window, cx| {
                                this.open_action(Action::Remove, true, window, cx)
                            }))
                            .on_action(cx.listener(|this, _: &UpdateSelection, window, cx| {
                                this.open_action(Action::Update, true, window, cx)
                            }))
                            .on_action(cx.listener(|this, _: &WriteSelection, window, cx| {
                                this.open_action(Action::Write, true, window, cx)
                            }))
                            .on_action(cx.listener(|this, _: &MoveSelection, window, cx| {
                                this.open_action(Action::Move, true, window, cx)
                            }))
                            .on_action(cx.listener(|this, _: &RevealSelection, _, cx| {
                                if let Some(p) = this.detail.as_ref().and_then(|d| d.path.clone()) {
                                    this.reveal(&p, cx);
                                }
                            }))
                            .child(list),
                    ),
            )
            .child(
                div()
                    .w(px(480.))
                    .flex_none()
                    .h_full()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(self.render_detail(window, cx)),
            )
    }

    fn render_import(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.import;
        let theme = cx.theme().clone();
        let choice = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      cx: &mut Context<Self>,
                      set: fn(&mut ImportForm)| {
            Button::new(id)
                .label(label)
                .small()
                .when(on, |b| b.primary())
                .when(!on, |b| b.outline())
                .on_click(cx.listener(move |this, _, _, cx| {
                    set(&mut this.import);
                    cx.notify();
                }))
        };
        let check = |id: &'static str,
                     label: &'static str,
                     on: bool,
                     cx: &mut Context<Self>,
                     set: fn(&mut ImportForm, bool)| {
            Checkbox::new(id)
                .label(label)
                .checked(on)
                .on_click(cx.listener(move |this, v: &bool, _, cx| {
                    set(&mut this.import, *v);
                    cx.notify();
                }))
        };
        let args = f.args(cx);
        let can_start = args.len() > 1 && !f.paths.read(cx).value().trim().is_empty();
        let preview = runner::shell_join(&args);
        let (mode, write, incremental) = (f.mode, f.write, f.incremental);
        let (autotag, singletons, group, flat, timid, quiet, pretend, scratch) = (
            f.autotag,
            f.singletons,
            f.group_albums,
            f.flat,
            f.timid,
            f.quiet,
            f.pretend,
            f.from_scratch,
        );
        v_flex()
            .id("import")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_5()
            .max_w(px(900.))
            .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Import music"))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Runs beet import in the console. MusicBrainz matches and other questions show up there with buttons for the answers."),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(section_title("Source", cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&f.paths)))
                            .child(
                                Button::new("browse")
                                    .label("Browse…")
                                    .on_click(cx.listener(|this, _, window, cx| this.browse_import(window, cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title("Files", cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(choice("m-def", "Config default", mode == FileMode::Default, cx, |f| f.mode = FileMode::Default))
                            .child(choice("m-copy", "Copy into library", mode == FileMode::Copy, cx, |f| f.mode = FileMode::Copy))
                            .child(choice("m-move", "Move into library", mode == FileMode::Move, cx, |f| f.mode = FileMode::Move))
                            .child(choice("m-keep", "Leave in place", mode == FileMode::InPlace, cx, |f| f.mode = FileMode::InPlace)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().w(px(110.)).text_sm().child("Write tags"))
                            .child(choice("w-def", "Default", write == Tri::Default, cx, |f| f.write = Tri::Default))
                            .child(choice("w-yes", "Yes", write == Tri::Yes, cx, |f| f.write = Tri::Yes))
                            .child(choice("w-no", "No", write == Tri::No, cx, |f| f.write = Tri::No)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().w(px(110.)).text_sm().child("Incremental"))
                            .child(choice("i-def", "Default", incremental == Tri::Default, cx, |f| f.incremental = Tri::Default))
                            .child(choice("i-yes", "Skip imported", incremental == Tri::Yes, cx, |f| f.incremental = Tri::Yes))
                            .child(choice("i-no", "Re-import", incremental == Tri::No, cx, |f| f.incremental = Tri::No)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title("Matching", cx))
                    .child(check("autotag", "Look up metadata (autotag)", autotag, cx, |f, v| f.autotag = v))
                    .child(check("singletons", "Import tracks as singletons, not albums", singletons, cx, |f, v| f.singletons = v))
                    .child(check("group", "Group tracks by album tags, not by folder", group, cx, |f, v| f.group_albums = v))
                    .child(check("flat", "Treat each folder tree as one album (flat)", flat, cx, |f, v| f.flat = v))
                    .child(check("timid", "Always ask, even for strong matches (timid)", timid, cx, |f, v| f.timid = v))
                    .child(check("quiet", "Never ask: apply strong matches, skip the rest (quiet)", quiet, cx, |f, v| f.quiet = v))
                    .child(check("pretend", "Pretend: only list what would be imported", pretend, cx, |f, v| f.pretend = v))
                    .child(check("scratch", "Discard existing tags before applying (from scratch)", scratch, cx, |f, v| f.from_scratch = v))
                    .child(Input::new(&f.search_id))
                    .child(Input::new(&f.set)),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(format!("beet {preview}")),
                    )
                    .child(
                        h_flex().child(
                            Button::new("start-import")
                                .label("Start import")
                                .primary()
                                .disabled(!can_start)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let args = this.import.args(cx);
                                    let title = format!("import {}", this.import.paths.read(cx).value());
                                    this.start(title, args, true, cx);
                                })),
                        ),
                    ),
            )
    }

    fn render_console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active_session;
        let list = self
            .runner
            .sessions
            .iter()
            .rev()
            .map(|s| {
                let id = s.id;
                let waiting = s.prompt().is_some();
                let status_color = match &s.status {
                    runner::Status::Running if waiting => theme.warning,
                    runner::Status::Running => theme.info,
                    _ if s.succeeded() => theme.success,
                    _ => theme.danger,
                };
                v_flex()
                    .id(("session", id))
                    .px_3()
                    .py_2()
                    .gap_0p5()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(active == Some(id), |d| d.bg(theme.list_active))
                    .hover(|d| d.bg(theme.list_hover))
                    .child(
                        div()
                            .text_sm()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(s.title.clone()),
                    )
                    .child(div().text_xs().text_color(status_color).child(if waiting {
                        "waiting for an answer".to_string()
                    } else {
                        s.status_text()
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_session = Some(id);
                        this.follow_output = true;
                        this.console_scroll.scroll_to_bottom();
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        let output = match active.and_then(|i| self.runner.sessions.get(i)) {
            Some(s) => self.render_session(s, cx).into_any_element(),
            None => empty_state(
                "Commands you run show their output here. Type any beet command above, e.g. `stats`, `ls -a`, or a plugin command.",
                cx,
            )
            .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .child("beet"),
                    )
                    .child(div().flex_1().child(Input::new(&self.command_input)))
                    .child(
                        Button::new("run-cmd")
                            .label("Run")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.command_input.read(cx).value().to_string();
                                if this.run_typed_command(&text, cx) {
                                    this.command_input
                                        .update(cx, |s, cx| s.set_value("", window, cx));
                                }
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("sessions")
                            .w(px(240.))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(theme.border)
                            .children(list),
                    )
                    .child(div().flex_1().h_full().min_w_0().child(output)),
            )
    }

    fn render_session(&self, s: &Session, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let prompt = s.prompt();
        let lines = s
            .lines
            .iter()
            .chain(std::iter::once(&s.partial))
            .map(|l| div().min_h(px(18.)).child(styled_line(l, cx)))
            .collect::<Vec<_>>();
        let elapsed = s
            .finished
            .unwrap_or_else(std::time::Instant::now)
            .duration_since(s.started)
            .as_secs_f32();
        let id = s.id;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_family(theme.mono_font_family.clone())
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!("beet {}", runner::shell_join(&s.args))),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        if s.running() {
                            s.status_text()
                        } else {
                            format!("{} · {elapsed:.1}s", s.status_text())
                        },
                    ))
                    .when(s.running(), |d| {
                        d.child(
                            Button::new("kill")
                                .label("Stop")
                                .xsmall()
                                .danger()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.runner.sessions[id].kill();
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!s.running(), |d| {
                        let args = s.args.clone();
                        let title = s.title.clone();
                        let mutates = s.mutates;
                        d.child(Button::new("rerun").label("Run again").xsmall().on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.start(title.clone(), args.clone(), mutates, cx)
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .id(("output", id))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.console_scroll)
                    .on_scroll(cx.listener(|this, _, _, cx| {
                        // Scrolling up stops following; reaching the end resumes.
                        let handle = &this.console_scroll;
                        let max = handle.max_offset().y;
                        this.follow_output = -handle.offset().y >= max - px(4.);
                        cx.notify();
                    }))
                    .p_3()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .bg(theme.background)
                    .children(lines),
            )
            .when(s.running(), |d| {
                d.child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .bg(theme.secondary)
                        .when_some(prompt, |d, p| {
                            d.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .when(p.numbers > 0, |d| {
                                        d.children((1..=p.numbers).map(|n| {
                                            Button::new(("num", n))
                                                .label(n.to_string())
                                                .small()
                                                .outline()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.answer(&n.to_string(), cx)
                                                }))
                                        }))
                                    })
                                    .children(p.choices.into_iter().enumerate().map(|(i, c)| {
                                        let answer = c.answer.clone();
                                        Button::new(("choice", i))
                                            .label(c.label.clone())
                                            .small()
                                            .when(c.default, |b| b.primary())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.answer(&answer, cx)
                                            }))
                                    })),
                            )
                        })
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(Input::new(&self.console_input)))
                                .child(Button::new("send").label("Send").on_click(cx.listener(
                                    |this, _, window, cx| {
                                        let text = this.console_input.read(cx).value().to_string();
                                        this.answer(&text, cx);
                                        this.console_input
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                    },
                                ))),
                        ),
                )
            })
    }

    // -- plugin toolbox ---------------------------------------------------

    fn plugin_enabled(&self, name: &str) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| i.enabled.iter().any(|p| p == name))
    }

    /// The plugins the search and the chip let through.
    fn shown_plugins(&self, cx: &App) -> Vec<&catalog::PluginInfo> {
        let search = self.plugin_search.read(cx).value().to_string();
        self.catalog
            .iter()
            .filter(|p| match self.plugin_filter {
                None => true,
                Some("Enabled") => self.plugin_enabled(p.name),
                Some(c) => p.category == c,
            })
            .filter(|p| catalog::matches(p, &search))
            .collect()
    }

    /// Turn a plugin on or off: rewrite the `plugins:` line of the user's
    /// config and reload.
    fn set_plugin_enabled(
        &mut self,
        name: &str,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(info) = &self.info else {
            return;
        };
        let editor_dirty = self.config_path.is_some()
            && self.config_editor.read(cx).value().as_ref() != self.config_saved.as_str();
        if editor_dirty {
            self.plugin_message = Some((
                false,
                "The configuration has unsaved edits: save or revert them first.".into(),
            ));
            cx.notify();
            return;
        }
        let path = info.user_config.clone();
        let mut names = info.enabled.clone();
        if on {
            if !names.iter().any(|p| p == name) {
                names.push(name.to_string());
            }
        } else {
            names.retain(|p| p != name);
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let new = catalog::set_plugins(&text, &names);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, new));
        match written {
            Ok(()) => {
                self.plugin_message = Some((
                    true,
                    format!(
                        "{} {name} in {}",
                        if on { "Enabled" } else { "Disabled" },
                        path.display()
                    ),
                ));
                if let Some(info) = &mut self.info {
                    info.enabled = names;
                }
                if self.config_path.is_some() {
                    self.load_config_file(window, cx, true);
                }
                self.reload(cx);
            }
            Err(e) => self.plugin_message = Some((false, format!("Couldn't save: {e}"))),
        }
        cx.notify();
    }

    /// Put a plugin's default settings at the end of the config editor to
    /// change and save.
    fn customize_plugin(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(defaults) = self
            .catalog
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.defaults)
        else {
            return;
        };
        self.show(Page::Config, window, cx);
        let text = self.config_editor.read(cx).value().to_string();
        let has_section = text
            .lines()
            .any(|l| l.strip_prefix(name).is_some_and(|r| r.starts_with(':')));
        self.config_message = Some(if has_section {
            (true, format!("{name} already has settings in this file."))
        } else {
            let mut new = text;
            if !new.is_empty() && !new.ends_with('\n') {
                new.push('\n');
            }
            if !new.is_empty() {
                new.push('\n');
            }
            new.push_str(defaults);
            new.push('\n');
            self.config_editor
                .update(cx, |s, cx| s.set_value(new, window, cx));
            (
                true,
                format!(
                    "Added {name}'s default settings at the end: change what you need, then Save (Ctrl+S)."
                ),
            )
        });
        cx.notify();
    }

    /// A plugin command on the library selection, everything, or in the
    /// console to add options.
    fn run_plugin_command(
        &mut self,
        cmd: &str,
        target: CommandTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = vec![cmd.to_string()];
        match target {
            CommandTarget::Selection => {
                self.run_quick(&base, cx);
                self.show(Page::Console, window, cx);
            }
            CommandTarget::Everything => {
                self.start(runner::shell_join(&base), base, true, cx);
            }
            CommandTarget::Help => {
                let args = vec![cmd.to_string(), "-h".into()];
                self.start(runner::shell_join(&args), args, false, cx);
            }
            CommandTarget::Console => {
                self.show(Page::Console, window, cx);
                self.command_input.update(cx, |s, cx| {
                    s.set_value(format!("{cmd} "), window, cx);
                    s.focus(window, cx);
                });
            }
        }
    }

    fn render_plugins(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let shown: Vec<catalog::PluginInfo> = self.shown_plugins(cx).into_iter().cloned().collect();
        let enabled_count = self.info.as_ref().map_or(0, |i| i.enabled.len());
        let app = cx.entity().downgrade();

        let chip = |id: SharedString,
                    label: String,
                    value: Option<&'static str>,
                    on: bool,
                    cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_filter = value;
                    cx.notify();
                }))
        };
        let mut chips = vec![
            chip(
                "chip-all".into(),
                format!("All {}", self.catalog.len()),
                None,
                self.plugin_filter.is_none(),
                cx,
            ),
            chip(
                "chip-enabled".into(),
                format!("Enabled {enabled_count}"),
                Some("Enabled"),
                self.plugin_filter == Some("Enabled"),
                cx,
            ),
        ];
        for &c in catalog::CATEGORIES {
            let n = self.catalog.iter().filter(|p| p.category == c).count();
            chips.push(chip(
                SharedString::from(format!("chip-{c}")),
                format!("{c} {n}"),
                Some(c),
                self.plugin_filter == Some(c),
                cx,
            ));
        }

        let selected = self.plugin_selected;
        let rows = shown.iter().map(|p| {
            let name = p.name;
            let on = self.plugin_enabled(name);
            let app = app.clone();
            h_flex()
                .id(SharedString::from(format!("plugin-{name}")))
                .w_full()
                .px_3()
                .py_2()
                .gap_3()
                .border_b_1()
                .border_color(theme.border)
                .cursor_pointer()
                .when(selected == Some(name), |d| d.bg(theme.list_active))
                .hover(|d| d.bg(theme.list_hover))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_weight(FontWeight::MEDIUM).child(name))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(p.category),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(p.about),
                        ),
                )
                .child(
                    gpui_kit::component::switch::Switch::new(SharedString::from(format!(
                        "switch-{name}"
                    )))
                    .checked(on)
                    .on_click(move |v: &bool, window, cx| {
                        let v = *v;
                        app.update(cx, |this, cx| this.set_plugin_enabled(name, v, window, cx))
                            .ok();
                    }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_selected = Some(name);
                    cx.notify();
                }))
        });

        let list = v_flex()
            .flex_1()
            .h_full()
            .min_w(px(380.))
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.plugin_search)
                                        .prefix(Icon::new(IconName::Search).small())
                                        .cleanable(true),
                                ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} shown", shown.len())),
                            ),
                    )
                    .child(h_flex().flex_wrap().gap_1().children(chips))
                    .children(self.plugin_message.clone().map(|(ok, m)| {
                        div()
                            .text_sm()
                            .text_color(if ok { theme.success } else { theme.danger })
                            .child(m)
                    })),
            )
            .child(
                v_flex()
                    .id("plugin-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(shown.is_empty(), |d| {
                        d.child(empty_state(
                            "No plugin matches. Try other words, or the All chip.",
                            cx,
                        ))
                    }),
            );

        h_flex().size_full().child(list).child(
            div()
                .w(px(480.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(theme.border)
                .child(self.render_plugin_detail(cx)),
        )
    }

    fn render_plugin_detail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(p) = self
            .plugin_selected
            .and_then(|n| self.catalog.iter().find(|p| p.name == n))
            .cloned()
        else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_6()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child("Pick a plugin to see what it does and its commands.")
                .child(
                    div()
                        .text_sm()
                        .child("Switches turn plugins on and off in your config.yaml."),
                )
                .into_any_element();
        };
        let name = p.name;
        let on = self.plugin_enabled(name);
        let loaded = self
            .info
            .as_ref()
            .is_some_and(|i| i.plugins.iter().any(|x| x == name));
        let failed = on && !loaded && name != "musicbrainz" && self.info.is_some();
        let selection = self.marked.len().max(usize::from(self.selected.is_some()));
        let what = match self.selected {
            Some((Kind::Album, _)) => "albums",
            _ => "tracks",
        };

        let commands = p.commands.iter().map(|c| {
            let cmd = c.name.clone();
            let (c1, c2, c3, c4) = (cmd.clone(), cmd.clone(), cmd.clone(), cmd.clone());
            v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex().gap_2().child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("beet {cmd}")),
                    ),
                )
                .when(!c.about.is_empty(), |d| {
                    d.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(c.about.clone()),
                    )
                })
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .when(c.takes_query, |d| {
                            d.child(
                                Button::new(SharedString::from(format!("run-sel-{cmd}")))
                                    .label(if selection > 0 {
                                        format!("Run on selection ({selection} {what})")
                                    } else {
                                        "Run on selection".to_string()
                                    })
                                    .xsmall()
                                    .primary()
                                    .disabled(!on || selection == 0)
                                    .tooltip("Select albums or tracks in the library first")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.run_plugin_command(
                                            &c1,
                                            CommandTarget::Selection,
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                        })
                        .child(
                            Button::new(SharedString::from(format!("run-all-{cmd}")))
                                .label(if c.takes_query {
                                    "Run on everything"
                                } else {
                                    "Run"
                                })
                                .xsmall()
                                .outline()
                                .disabled(!on)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_plugin_command(
                                        &c2,
                                        CommandTarget::Everything,
                                        window,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("run-con-{cmd}")))
                                .label("With options…")
                                .xsmall()
                                .ghost()
                                .disabled(!on)
                                .tooltip("Type the command with options in the console")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_plugin_command(&c3, CommandTarget::Console, window, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("run-help-{cmd}")))
                                .label("Options")
                                .xsmall()
                                .ghost()
                                .disabled(!on)
                                .tooltip("Show the command's options (-h)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_plugin_command(&c4, CommandTarget::Help, window, cx)
                                })),
                        ),
                )
        });

        v_flex()
            .id("plugin-detail")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(p.category.to_uppercase()))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_1().text_xl().font_weight(FontWeight::BOLD).child(name))
                            .child(
                                Button::new("plugin-toggle")
                                    .label(if on { "Disable" } else { "Enable" })
                                    .small()
                                    .when(!on, |b| b.primary())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_plugin_enabled(name, !on, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if on { theme.success } else { theme.muted_foreground })
                            .child(if on { "Enabled" } else { "Not enabled" }),
                    )
                    .when(failed, |d| {
                        d.child(div().text_sm().text_color(theme.warning).child(
                            "Enabled, but it didn't load: it may need settings (an API key) or a program. Run a command to see the error.",
                        ))
                    }),
            )
            .child(div().child(if p.about.is_empty() { "No description." } else { p.about }))
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title(
                        if p.commands.is_empty() {
                            "No commands: it works on its own (on import, or as a query or path feature)".to_string()
                        } else {
                            "Commands".to_string()
                        },
                        cx,
                    ))
                    .when(!on && !p.commands.is_empty(), |d| {
                        d.child(div().text_sm().text_color(theme.muted_foreground).child("Enable the plugin to use its commands."))
                    })
                    .children(commands),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(section_title("Settings", cx)))
                            .when(!p.defaults.is_empty(), |d| {
                                d.child(
                                    Button::new("plugin-customize")
                                        .label("Customize…")
                                        .xsmall()
                                        .outline()
                                        .tooltip("Copy these defaults into config.yaml to change them")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.customize_plugin(name, window, cx)
                                        })),
                                )
                            }),
                    )
                    .child(if p.defaults.is_empty() {
                        div().text_sm().text_color(theme.muted_foreground).child("No settings.")
                    } else {
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .overflow_hidden()
                            .children(p.defaults.lines().map(|l| {
                                div()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(l.to_string())
                            }))
                    }),
            )
            .into_any_element()
    }

    fn render_commands(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let groups: [(&str, Vec<CommandButton>); 3] = [
            (
                "Library",
                vec![
                    ("stats", "Statistics", vec!["stats"]),
                    ("stats-e", "Exact statistics", vec!["stats", "-e"]),
                    ("fields", "Available fields", vec!["fields"]),
                    ("ls-a", "List albums", vec!["ls", "-a"]),
                ],
            ),
            (
                "Maintenance",
                vec![
                    (
                        "update-all",
                        "Update everything from files (pretend)",
                        vec!["update", "-p"],
                    ),
                    (
                        "move-all",
                        "Move everything into place (pretend)",
                        vec!["move", "-p"],
                    ),
                    (
                        "write-all",
                        "Write tags to all files (pretend)",
                        vec!["write", "-p"],
                    ),
                ],
            ),
            (
                "About",
                vec![
                    ("version", "Version and plugins", vec!["version"]),
                    ("config", "Effective configuration", vec!["config"]),
                    ("config-p", "Configuration files", vec!["config", "-p"]),
                    ("help", "All commands", vec!["help"]),
                ],
            ),
        ];
        let plugins = self
            .info
            .as_ref()
            .map(|i| i.plugins.clone())
            .unwrap_or_default();
        v_flex()
            .id("commands")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_5()
            .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Commands"))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Every beet command, including plugin commands, can be run from the Console page. These are common ones."),
            )
            .children(groups.into_iter().map(|(title, cmds)| {
                v_flex()
                    .gap_2()
                    .child(section_title(title, cx))
                    .child(h_flex().flex_wrap().gap_2().children(cmds.into_iter().map(
                        |(id, label, args)| {
                            let args: Vec<String> = args.into_iter().map(String::from).collect();
                            let read_only = !matches!(args[0].as_str(), "update" | "move" | "write")
                                || args.iter().any(|a| a == "-p");
                            Button::new(id).label(label).outline().on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.start(runner::shell_join(&args), args.clone(), !read_only, cx)
                                },
                            ))
                        },
                    )))
            }))
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title("Loaded plugins", cx))
                    .child(if plugins.is_empty() {
                        div()
                            .text_color(theme.muted_foreground)
                            .child("None. Add them to `plugins:` on the Configuration page.")
                            .into_any_element()
                    } else {
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .children(plugins.into_iter().map(|p| {
                                let help = vec!["help".to_string(), p.clone()];
                                Button::new(SharedString::from(format!("plugin-{p}")))
                                    .label(p)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start(runner::shell_join(&help), help.clone(), false, cx)
                                    }))
                            }))
                            .into_any_element()
                    }),
            )
    }

    fn render_config(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.config_path.is_none() && self.info.is_some() {
            self.load_config_file(window, cx, false);
        }
        let theme = cx.theme().clone();
        let dirty = self.config_editor.read(cx).value().as_ref() != self.config_saved.as_str();
        let sources = self
            .info
            .as_ref()
            .map(|i| i.config_sources.clone())
            .unwrap_or_default();
        v_flex()
            .size_full()
            .key_context("ConfigPage")
            .on_action(cx.listener(|this, _: &SaveConfig, _, cx| this.save_config(cx)))
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Configuration"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(theme.muted_foreground)
                                    .child(match &self.config_path {
                                        Some(p) => format!(
                                            "{}{}",
                                            p.display(),
                                            if p.exists() { "" } else { " (new)" }
                                        ),
                                        None => "Opening…".into(),
                                    }),
                            ),
                    )
                    .child(
                        Button::new("cfg-revert")
                            .label("Revert")
                            .disabled(!dirty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_config_file(window, cx, true);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("cfg-effective")
                            .label("Show effective config")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start("config".into(), vec!["config".into()], false, cx)
                            })),
                    )
                    .child(
                        Button::new("cfg-save")
                            .label("Save and reload")
                            .primary()
                            .disabled(!dirty)
                            .on_click(cx.listener(|this, _, _, cx| this.save_config(cx))),
                    ),
            )
            .children(self.config_message.clone().map(|(ok, m)| {
                div()
                    .text_sm()
                    .text_color(if ok { theme.success } else { theme.danger })
                    .child(m)
            }))
            .children(self.open_error.clone().map(|e| {
                div().text_sm().text_color(theme.danger).child(format!(
                    "The library could not open with this configuration: {e}"
                ))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(Editor::new(&self.config_editor).h_full()),
            )
            .when(sources.len() > 1, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "Sources, highest priority first: {}",
                            sources
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                )
            })
    }
}

/// A command button: element id, label, `beet` arguments.
type CommandButton = (&'static str, &'static str, Vec<&'static str>);

/// The background of a selected row in a multiple selection: stronger than
/// the theme's hover so a scattered selection stands out.
fn selection_bg(cx: &App) -> Hsla {
    cx.theme().primary.opacity(0.16)
}

fn empty_state(text: &str, cx: &App) -> impl IntoElement {
    div()
        .p_8()
        .text_center()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

impl Render for BeetsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Albums | Page::Tracks => self.render_library(window, cx).into_any_element(),
            Page::Import => self.render_import(cx).into_any_element(),
            Page::Console => self.render_console(cx).into_any_element(),
            Page::Commands => self.render_commands(cx).into_any_element(),
            Page::Plugins => self.render_plugins(cx).into_any_element(),
            Page::Config => self.render_config(window, cx).into_any_element(),
        };
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                if this.page.kind().is_none() {
                    this.show(Page::Albums, window, cx);
                }
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &ShowAlbums, window, cx| {
                this.show(Page::Albums, window, cx);
                this.focus_list(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowTracks, window, cx| {
                this.show(Page::Tracks, window, cx);
                this.focus_list(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ShowImport, window, cx| this.show(Page::Import, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowPlugins, window, cx| {
                this.show(Page::Plugins, window, cx);
                this.plugin_search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ShowCommands, window, cx| {
                this.show(Page::Commands, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowConfig, window, cx| this.show(Page::Config, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.show_shortcuts(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowConsole, window, cx| {
                    this.show(Page::Console, window, cx)
                }),
            )
            .child(self.render_sidebar(cx))
            .child(div().flex_1().h_full().min_w_0().child(page))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has its own `test` attribute.
    use super::{ids_query, runner};

    #[test]
    fn selection_queries_are_one_argument() {
        assert_eq!(ids_query("id", &[7]), "id:7");
        assert_eq!(ids_query("album_id", &[1, 3, 12]), "album_id::^(1|3|12)$");
    }

    #[test]
    fn long_commands_are_shortened_for_display() {
        let ids: Vec<i64> = (1..2000).collect();
        let args = vec!["modify".to_string(), ids_query("id", &ids)];
        let short = runner::shell_join_short(&args);
        assert!(short.chars().count() <= 301);
        assert!(short.ends_with('…'));
        assert_eq!(runner::shell_join_short(&["ls".into()]), "ls");
    }
}
