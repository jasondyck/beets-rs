//! Terminal helpers: colors and beets-style prompts.

use crate::config::Config;
use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde_yaml::Value as Y;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{LazyLock, RwLock};

// ANSI colours as in beets' `beets.util.color`.

const ESC: &str = "\x1b";
pub const RESET_COLOR: &str = "\x1b[39;49;00m";

/// beets' `ColorName`s, the keys of `ui.colors`.
pub const COLOR_NAMES: &[&str] = &[
    "text_success",
    "text_warning",
    "text_error",
    "text_highlight",
    "text_highlight_minor",
    "action_default",
    "action",
    "text_faint",
    "import_path",
    "import_path_items",
    "action_description",
    "changed",
    "text_diff_added",
    "text_diff_removed",
];

/// beets' `LEGACY_COLORS`: single colour names from older configs.
const LEGACY_COLORS: &[(&str, &[&str])] = &[
    ("black", &["black"]),
    ("darkred", &["red"]),
    ("darkgreen", &["green"]),
    ("brown", &["yellow"]),
    ("darkyellow", &["yellow"]),
    ("darkblue", &["blue"]),
    ("purple", &["magenta"]),
    ("darkmagenta", &["magenta"]),
    ("teal", &["cyan"]),
    ("darkcyan", &["cyan"]),
    ("lightgray", &["white"]),
    ("darkgray", &["bold", "black"]),
    ("red", &["bold", "red"]),
    ("green", &["bold", "green"]),
    ("yellow", &["bold", "yellow"]),
    ("blue", &["bold", "blue"]),
    ("fuchsia", &["bold", "magenta"]),
    ("magenta", &["bold", "magenta"]),
    ("turquoise", &["bold", "cyan"]),
    ("cyan", &["bold", "cyan"]),
    ("white", &["bold", "white"]),
];

/// beets' `CODE_BY_COLOR`: SGR codes for styles and colours.
const CODE_BY_COLOR: &[(&str, u8)] = &[
    ("normal", 0),
    ("bold", 1),
    ("faint", 2),
    ("italic", 3),
    ("underline", 4),
    ("blink_slow", 5),
    ("blink_rapid", 6),
    ("inverse", 7),
    ("conceal", 8),
    ("crossed_out", 9),
    ("black", 30),
    ("red", 31),
    ("green", 32),
    ("yellow", 33),
    ("blue", 34),
    ("magenta", 35),
    ("cyan", 36),
    ("white", 37),
    ("bright_black", 90),
    ("bright_red", 91),
    ("bright_green", 92),
    ("bright_yellow", 93),
    ("bright_blue", 94),
    ("bright_magenta", 95),
    ("bright_cyan", 96),
    ("bright_white", 97),
    ("bg_black", 40),
    ("bg_red", 41),
    ("bg_green", 42),
    ("bg_yellow", 43),
    ("bg_blue", 44),
    ("bg_magenta", 45),
    ("bg_cyan", 46),
    ("bg_white", 47),
    ("bg_bright_black", 100),
    ("bg_bright_red", 101),
    ("bg_bright_green", 102),
    ("bg_bright_yellow", 103),
    ("bg_bright_blue", 104),
    ("bg_bright_magenta", 105),
    ("bg_bright_cyan", 106),
    ("bg_bright_white", 107),
];

/// Colour settings for this run: None (the default, e.g. in tests) means
/// no colour.
static COLORS: RwLock<Option<HashMap<&'static str, String>>> = RwLock::new(None);

/// beets' `get_color_config`: each `ui.colors` entry, a legacy colour name
/// or a list of style/colour names, as an SGR code string ("1;32").
pub fn color_codes(cfg: &Config) -> Result<HashMap<&'static str, String>> {
    let mut out = HashMap::new();
    for name in COLOR_NAMES {
        let key = format!("ui.colors.{name}");
        let names: Vec<String> = match cfg.get(&key) {
            Some(Y::String(s)) => match LEGACY_COLORS.iter().find(|(n, _)| n == s) {
                Some((_, c)) => c.iter().map(|c| c.to_string()).collect(),
                None => bail!(
                    "{key}: must be one of {} or a list of {}, not {s:?}",
                    LEGACY_COLORS
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", "),
                    "styles and colors"
                ),
            },
            Some(Y::Sequence(seq)) => seq
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            _ => bail!("{key}: must be a color name or a list of color names"),
        };
        let codes: Vec<String> = names
            .iter()
            .map(|n| {
                CODE_BY_COLOR
                    .iter()
                    .find(|(c, _)| c == n)
                    .map(|(_, code)| code.to_string())
                    .ok_or_else(|| anyhow!("{key}: unknown color {n:?}"))
            })
            .collect::<Result<_>>()?;
        out.insert(*name, codes.join(";"));
    }
    Ok(out)
}

/// Set up colours from `ui.color`/`ui.colors` (beets: colour unless
/// `ui.color` is off or `NO_COLOR` is set; no terminal check).
pub fn configure(cfg: &Config) -> Result<()> {
    let codes = color_codes(cfg)?;
    let on = color_on(cfg, std::env::var_os("NO_COLOR").is_some());
    *COLORS.write().unwrap() = on.then_some(codes);
    Ok(())
}

fn color_on(cfg: &Config, no_color: bool) -> bool {
    cfg.bool("ui.color") && !no_color
}

/// beets' `colorize`: `text` in the colour configured for `name` (one of
/// `COLOR_NAMES`), or as is when colour is off.
pub fn colorize(name: &str, text: &str) -> String {
    colorize_with(COLORS.read().unwrap().as_ref(), name, text)
}

fn colorize_with(codes: Option<&HashMap<&'static str, String>>, name: &str, text: &str) -> String {
    match codes.and_then(|c| c.get(name)) {
        Some(code) => format!("{ESC}[{code}m{text}{RESET_COLOR}"),
        None => text.to_string(),
    }
}

static ANSI_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[;0-9]*m").unwrap());

/// beets' `uncolorize`.
pub fn uncolorize(s: &str) -> String {
    ANSI_CODE.replace_all(s, "").into_owned()
}

/// beets' `color_len`: visible length, ignoring colour codes.
pub fn color_len(s: &str) -> usize {
    uncolorize(s).chars().count()
}

/// beets' `color_split`: split a coloured string after `index` visible
/// characters, closing and reopening the colour around the split.
pub fn color_split(s: &str, index: usize) -> (String, String) {
    let mut parts = vec![];
    let mut last = 0;
    for m in ANSI_CODE.find_iter(s) {
        parts.push(&s[last..m.start()]);
        parts.push(m.as_str());
        last = m.end();
    }
    parts.push(&s[last..]);
    let (mut pre, mut post) = (String::new(), String::new());
    let mut length = 0;
    let mut found_code: Option<&str> = None;
    let mut found_split = false;
    for part in parts {
        length += color_len(part);
        if found_split {
            post += part;
        } else if ANSI_CODE.is_match(part) {
            found_code = (part != RESET_COLOR).then_some(part);
            pre += part;
        } else if index < length {
            let split = index - (length - color_len(part));
            let at = part
                .char_indices()
                .nth(split)
                .map_or(part.len(), |(i, _)| i);
            found_split = true;
            match found_code {
                Some(code) => {
                    pre += &part[..at];
                    pre += RESET_COLOR;
                    post += code;
                    post += &part[at..];
                }
                None => {
                    pre += &part[..at];
                    post += &part[at..];
                }
            }
        } else {
            pre += part;
        }
    }
    (pre, post)
}

/// Scripted answers for tests: while set, `read_line` takes its answers
/// from here (None once they run out: end of input), running each one's
/// hook first, and keeps the prompts it was asked.
#[cfg(test)]
pub mod script {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    pub type Hook = Box<dyn FnOnce()>;
    type Queue = VecDeque<(String, Option<Hook>)>;

    thread_local! {
        static ANSWERS: RefCell<Option<Queue>> =
            const { RefCell::new(None) };
        static PROMPTS: RefCell<Vec<String>> = const { RefCell::new(vec![]) };
    }

    /// Answer the next prompts with `answers`, in order.
    pub fn set(answers: Vec<(&str, Option<Hook>)>) {
        let a = answers
            .into_iter()
            .map(|(s, h)| (s.to_string(), h))
            .collect();
        ANSWERS.with(|x| *x.borrow_mut() = Some(a));
        PROMPTS.with(|p| p.borrow_mut().clear());
    }

    /// The prompts asked since `set`.
    pub fn prompts() -> Vec<String> {
        PROMPTS.with(|p| p.borrow().clone())
    }

    /// Some(answer) while scripting (None inside: the answers ran out).
    pub(super) fn next(prompt: &str) -> Option<Option<String>> {
        let (answer, hook) = ANSWERS.with(|x| {
            let mut x = x.borrow_mut();
            let q = x.as_mut()?;
            Some(match q.pop_front() {
                Some((s, h)) => (Some(s), h),
                None => (None, None),
            })
        })?;
        PROMPTS.with(|p| p.borrow_mut().push(prompt.to_string()));
        if let Some(h) = hook {
            h();
        }
        Some(answer)
    }
}

pub fn read_line(prompt: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(answer) = script::next(prompt) {
        return answer;
    }
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        // beets' `input_`: a UserError, which ends the command (state
        // already saved stays saved, as when beets raises it).
        Ok(0) | Err(_) => {
            eprintln!("error: stdin stream ended while input required");
            std::process::exit(1);
        }
        Ok(_) => Some(line.trim().to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Letter(char),
    Number(usize),
}

/// beets' `input_options` with the prompt made from `opts` (e.g. "Apply",
/// "More candidates", "aBort": the key is the capital letter, else the
/// first letter not taken). With `numbers`, 1..=n are accepted too.
/// `default` is the answer to empty input; None requires an answer.
/// Returns None at the end of input.
pub fn options(opts: &[&str], numbers: Option<usize>, default: Option<Choice>) -> Option<Choice> {
    let require = default.is_none();
    prompt_options(opts, require, None, None, numbers.map(|n| (1, n)), default)
}

/// beets' `input_options` with an explicit prompt: the answer is the first
/// letter of the input, if it's one of `letters`; empty input picks the
/// first letter unless `require`; anything else asks again with
/// `fallback`. None at end of input.
pub fn input_options(
    letters: &[char],
    require: bool,
    prompt: &str,
    fallback: &str,
) -> Option<char> {
    let opts: Vec<String> = letters.iter().map(|c| c.to_string()).collect();
    let opts: Vec<&str> = opts.iter().map(String::as_str).collect();
    match prompt_options(&opts, require, Some(prompt), Some(fallback), None, None)? {
        Choice::Letter(c) => Some(c),
        Choice::Number(_) => None,
    }
}

/// The letter for each option, as beets picks them.
fn option_letters(opts: &[&str]) -> Vec<(char, usize)> {
    let mut taken: Vec<char> = vec![];
    let mut out = vec![];
    for opt in opts {
        let found = opt
            .char_indices()
            .find(|(_, c)| c.is_alphabetic() && c.is_uppercase())
            .or_else(|| {
                opt.char_indices()
                    .find(|(_, c)| c.is_alphabetic() && !taken.contains(&c.to_ascii_lowercase()))
            })
            .unwrap_or((0, opt.chars().next().unwrap_or('?')));
        taken.push(found.1.to_ascii_lowercase());
        out.push((found.1, found.0));
    }
    out
}

/// beets' generated `input_options` prompt: "➜ " and the options,
/// comma-separated with a final "?", wrapped at `max_width`, with the
/// default marked "[X]" and the number range first.
fn options_prompt(
    opts: &[&str],
    require: bool,
    numrange: Option<(usize, usize)>,
    default: Option<&Choice>,
    max_width: usize,
) -> String {
    let mut parts: Vec<(String, usize)> = vec![];
    if numrange.is_some() {
        match default {
            Some(Choice::Number(d)) if !require => {
                let tmpl = "# selection (default {})";
                parts.push((
                    format!(
                        "# selection (default {})",
                        colorize("action_default", &d.to_string())
                    ),
                    tmpl.len() - 2 + d.to_string().len(),
                ));
            }
            _ => parts.push(("# selection".into(), "# selection".len())),
        }
    }
    for (i, (opt, (letter, idx))) in opts.iter().zip(option_letters(opts)).enumerate() {
        let is_default = !require
            && match default {
                None => numrange.is_none() && i == 0,
                Some(Choice::Letter(d)) => letter.to_ascii_lowercase() == *d,
                Some(Choice::Number(_)) => false,
            };
        let up = letter.to_uppercase().to_string();
        let show = if is_default { format!("[{up}]") } else { up };
        let show = colorize(
            if is_default {
                "action_default"
            } else {
                "action"
            },
            &show,
        );
        let descr = if is_default {
            "action_default"
        } else {
            "action_description"
        };
        let rest = &opt[idx + letter.len_utf8()..];
        parts.push((
            format!(
                "{}{show}{}",
                colorize(descr, &opt[..idx]),
                colorize(descr, rest)
            ),
            opt.chars().count(),
        ));
    }
    let mut prompt = colorize("action", "\u{279c} ");
    let mut line_length = 0;
    let n = parts.len();
    for (i, (mut part, mut length)) in parts.into_iter().enumerate() {
        part += &colorize("action_description", if i == n - 1 { "?" } else { "," });
        length += 1;
        if line_length + length + 1 > max_width {
            prompt.push('\n');
            line_length = 0;
        }
        if line_length != 0 {
            part = format!(" {part}");
            length += 1;
        }
        prompt += &part;
        line_length += length;
    }
    prompt
}

/// beets' `input_options`, in full.
pub fn prompt_options(
    opts: &[&str],
    require: bool,
    prompt: Option<&str>,
    fallback: Option<&str>,
    numrange: Option<(usize, usize)>,
    default: Option<Choice>,
) -> Option<Choice> {
    let letters = option_letters(opts);
    let default_choice = if require {
        None
    } else {
        default.or_else(|| match numrange {
            Some((lo, _)) => Some(Choice::Number(lo)),
            None => letters
                .first()
                .map(|(c, _)| Choice::Letter(c.to_ascii_lowercase())),
        })
    };
    let prompt = match prompt {
        Some(p) => p.to_string(),
        None => options_prompt(opts, require, numrange, default_choice.as_ref(), 72),
    };
    let fallback = match fallback {
        Some(f) => f.to_string(),
        None => {
            let mut f = "Enter one of ".to_string();
            if let Some((lo, hi)) = numrange {
                f += &format!("{lo}-{hi}, ");
            }
            let shown: Vec<String> = letters
                .iter()
                .map(|(c, _)| c.to_uppercase().to_string())
                .collect();
            f += &format!("{}:", shown.join(", "));
            f
        }
    };
    let mut line = read_line(&format!("{prompt} "))?;
    loop {
        let answer = line.trim().to_lowercase();
        if answer.is_empty()
            && let Some(d) = &default_choice
        {
            return Some(match d {
                Choice::Letter(c) => Choice::Letter(*c),
                Choice::Number(n) => Choice::Number(*n),
            });
        }
        if let (Some((lo, hi)), Ok(n)) = (numrange, answer.parse::<usize>())
            && answer.bytes().all(|b| b.is_ascii_digit())
        {
            if (lo..=hi).contains(&n) {
                return Some(Choice::Number(n));
            }
        } else if let Some(c) = answer.chars().next()
            && letters.iter().any(|(l, _)| l.to_ascii_lowercase() == c)
        {
            return Some(Choice::Letter(c));
        }
        line = read_line(&format!("{fallback} "))?;
    }
}

/// beets' `input_select_objects`: ask "PROMPT? (Yes/no/select)" and return
/// all, none, or (select) the objects confirmed one by one, after showing
/// each with `show`.
pub fn select_objects<T: Clone>(prompt: &str, objs: &[T], show: impl FnMut(&T)) -> Vec<T> {
    select_objects_all(prompt, None, objs, show)
}

/// `select_objects` with beets' `prompt_all`: the first question uses it
/// instead of `prompt`.
pub fn select_objects_all<T: Clone>(
    prompt: &str,
    prompt_all: Option<&str>,
    objs: &[T],
    mut show: impl FnMut(&T),
) -> Vec<T> {
    let choice = input_options(
        &['y', 'n', 's'],
        false,
        &format!("{}? (Yes/no/select)", prompt_all.unwrap_or(prompt)),
        "Enter one of Y, N, S:",
    );
    println!();
    match choice {
        Some('y') => objs.to_vec(),
        Some('s') => {
            let mut out = vec![];
            for obj in objs {
                show(obj);
                match input_options(
                    &['y', 'n', 'q'],
                    true,
                    &format!("{prompt}? (yes/no/quit)"),
                    "Enter Y or N:",
                ) {
                    Some('y') => out.push(obj.clone()),
                    Some('n') => {}
                    _ => return out,
                }
            }
            out
        }
        _ => vec![],
    }
}

/// beets' `input_yn`: "y" or "n", defaulting to yes unless `require`.
/// False at end of input.
pub fn input_yn(prompt: &str, require: bool) -> bool {
    let fallback = format!(
        "{}{}",
        colorize("action", "\u{279c} "),
        colorize("action_description", "Enter Y or N:")
    );
    input_options(&['y', 'n'], require, prompt, &fallback) == Some('y')
}

pub fn yes_no(prompt: &str) -> bool {
    loop {
        match read_line(&format!("{prompt} (yes/no)? "))
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("y" | "yes") => return true,
            Some("n" | "no") | None => return false,
            _ => {}
        }
    }
}

/// beets' `ui.term_width`: `$COLUMNS` or the size of the terminal on
/// stdout (as Python's `shutil.get_terminal_size`), else `fallback`
/// (beets' `ui.terminal_width`, 80 by default).
pub fn term_width(fallback: usize) -> usize {
    if let Some(n) = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n;
    }
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ only writes a `winsize` into the struct we pass.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_col > 0
        {
            return ws.ws_col as usize;
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> Config {
        let mut c = Config::defaults();
        for (k, v) in serde_yaml::from_str::<serde_yaml::Mapping>(yaml).unwrap() {
            c.set(k.as_str().unwrap(), v);
        }
        c
    }

    #[test]
    fn default_colors_are_beets_codes() {
        let codes = color_codes(&Config::defaults()).unwrap();
        assert_eq!(codes.len(), COLOR_NAMES.len());
        assert_eq!(codes["text_success"], "1;32");
        assert_eq!(codes["text_faint"], "2");
        assert_eq!(codes["action_description"], "37");
        assert_eq!(
            colorize_with(Some(&codes), "text_error", "x"),
            "\x1b[1;31mx\x1b[39;49;00m"
        );
        assert_eq!(colorize_with(None, "text_error", "x"), "x");
    }

    #[test]
    fn colors_take_lists_and_legacy_names_and_reject_others() {
        let c = cfg(
            "ui.colors.text_success: [underline, bg_blue, bright_white]\nui.colors.changed: darkgray",
        );
        let codes = color_codes(&c).unwrap();
        assert_eq!(codes["text_success"], "4;44;97");
        assert_eq!(codes["changed"], "1;30");
        for bad in [
            "ui.colors.action: bold",
            "ui.colors.action: [blod]",
            "ui.colors.action: 3",
        ] {
            let e = color_codes(&cfg(bad)).unwrap_err().to_string();
            assert!(e.starts_with("ui.colors.action: "), "{e}");
        }
    }

    #[test]
    fn color_follows_ui_color_and_no_color() {
        assert!(color_on(&Config::defaults(), false));
        assert!(!color_on(&Config::defaults(), true));
        assert!(!color_on(&cfg("ui.color: no"), false));
    }

    #[test]
    fn color_len_and_split_ignore_codes() {
        let s = format!("ab\x1b[1;31mcdef{RESET_COLOR}gh");
        assert_eq!(color_len(&s), 8);
        assert_eq!(uncolorize(&s), "abcdefgh");
        let (a, b) = color_split(&s, 4);
        assert_eq!(a, format!("ab\x1b[1;31mcd{RESET_COLOR}"));
        assert_eq!(b, format!("\x1b[1;31mef{RESET_COLOR}gh"));
        assert_eq!(color_split("plain", 2), ("pl".into(), "ain".into()));
    }

    #[test]
    fn options_prompt_as_beets() {
        let opts = [
            "Apply",
            "More candidates",
            "Skip",
            "Use as-is",
            "as Tracks",
            "Group albums",
            "Enter search",
            "enter Id",
            "aBort",
        ];
        assert_eq!(
            options_prompt(&opts, false, None, Some(&Choice::Letter('a')), 72),
            "\u{279c} [A]pply, More candidates, Skip, Use as-is, as Tracks, Group albums,\nEnter search, enter Id, aBort?"
        );
        assert_eq!(
            options_prompt(
                &opts[2..5],
                false,
                Some((1, 3)),
                Some(&Choice::Number(1)),
                72
            ),
            "\u{279c} # selection (default 1), Skip, Use as-is, as Tracks?"
        );
        assert_eq!(
            options_prompt(&opts[2..5], true, Some((1, 3)), None, 72),
            "\u{279c} # selection, Skip, Use as-is, as Tracks?"
        );
        assert_eq!(
            option_letters(&["Skip new", "Keep all", "Remove old", "Merge all"])
                .iter()
                .map(|(c, _)| *c)
                .collect::<String>(),
            "SKRM"
        );
        // Without capitals, the first free letter.
        assert_eq!(
            option_letters(&["yes", "yeah"])
                .iter()
                .map(|(c, _)| *c)
                .collect::<String>(),
            "ye"
        );
    }
}
