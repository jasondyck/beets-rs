//! `beet` runs: every command the GUI starts (and every one typed in the
//! console) is this executable re-run as `beet-gui --beet ARGS…`, with its
//! output streamed into a session and its prompts answered from the UI.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The argument that turns this executable into `beet`.
pub const BEET_FLAG: &str = "--beet";

pub enum Event {
    Output { session: usize, bytes: Vec<u8> },
    Exit { session: usize, code: Option<i32> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Running,
    Exited(Option<i32>),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Style {
    /// An ANSI colour index (0–15) or a 256-colour index, or 24-bit RGB.
    pub color: Option<Color>,
    pub bold: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// A line of output: its text and the styled ranges in it.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub text: String,
    pub styles: Vec<(std::ops::Range<usize>, Style)>,
}

impl Line {
    fn push(&mut self, s: &str, style: Style) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        if style != Style::default() {
            self.styles.push((start..self.text.len(), style));
        }
    }
}

/// One answer the current prompt offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// What to send (a letter, "y"/"n", or "" for the default).
    pub answer: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    pub choices: Vec<Choice>,
    /// How many number buttons to offer (a candidate or menu entry); 0
    /// when the prompt takes no number.
    pub numbers: usize,
}

pub struct Session {
    pub id: usize,
    pub title: String,
    pub args: Vec<String>,
    pub lines: Vec<Line>,
    /// The line being written (no newline yet): a prompt, while running.
    pub partial: Line,
    pub status: Status,
    pub started: Instant,
    pub finished: Option<Instant>,
    /// Reload the library view when the run ends (it may have changed it).
    pub mutates: bool,
    child: Option<Arc<Mutex<Child>>>,
    stdin: Option<ChildStdin>,
    style: Style,
    /// Bytes of an escape sequence or a UTF-8 character split across reads.
    pending: Vec<u8>,
}

/// Output chunks queued for the UI before readers block.
const EVENT_BACKLOG: usize = 1024;

/// Lines kept per session; older ones are dropped.
const MAX_LINES: usize = 20_000;

impl Session {
    pub fn running(&self) -> bool {
        self.status == Status::Running
    }

    pub fn status_text(&self) -> String {
        match &self.status {
            Status::Running => "running".into(),
            Status::Exited(Some(0)) => "done".into(),
            Status::Exited(Some(c)) => format!("failed (exit {c})"),
            Status::Exited(None) => "stopped".into(),
            Status::Failed(e) => format!("could not start: {e}"),
        }
    }

    pub fn succeeded(&self) -> bool {
        self.status == Status::Exited(Some(0))
    }

    /// Send a line of input (an answer to a prompt).
    pub fn send(&mut self, text: &str) {
        // Echo it, as a terminal would.
        let mut line = std::mem::take(&mut self.partial);
        line.push(
            text,
            Style {
                bold: true,
                ..Style::default()
            },
        );
        self.push_line(line);
        if let Some(stdin) = &mut self.stdin {
            let ok = writeln!(stdin, "{text}")
                .and_then(|_| stdin.flush())
                .is_ok();
            if !ok {
                self.stdin = None;
            }
        }
    }

    pub fn kill(&mut self) {
        if let Some(child) = &self.child {
            lock(child).kill().ok();
        }
    }

    fn push_line(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            let extra = self.lines.len() - MAX_LINES;
            self.lines.drain(..extra);
        }
    }

    /// Feed raw output: split lines, apply SGR colours, drop other escapes.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        // Keep an incomplete UTF-8 tail for the next read.
        let valid = match std::str::from_utf8(&data) {
            Ok(_) => data.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => data.len(),
        };
        let tail = data.split_off(valid);
        let text = String::from_utf8_lossy(&data).into_owned();
        let rest = text.as_str();
        let mut run_start = 0;
        let mut chars = rest.char_indices().peekable();
        let mut cut: Option<usize> = None;
        while let Some((i, c)) = chars.next() {
            match c {
                '\n' => {
                    let style = self.style;
                    self.partial.push(&rest[run_start..i], style);
                    let line = std::mem::take(&mut self.partial);
                    self.push_line(line);
                    run_start = i + 1;
                }
                '\r' => {
                    // "\r\n" is a newline; a lone "\r" rewinds the line.
                    if chars.peek().map(|(_, c)| *c) != Some('\n') {
                        self.partial = Line::default();
                    } else {
                        let style = self.style;
                        self.partial.push(&rest[run_start..i], style);
                    }
                    run_start = i + 1;
                }
                '\x1b' => {
                    let style = self.style;
                    self.partial.push(&rest[run_start..i], style);
                    // Find the end of a CSI sequence: ESC [ params final.
                    let seq = &rest[i..];
                    if seq.len() < 2 {
                        cut = Some(i);
                        break;
                    }
                    if seq.as_bytes()[1] != b'[' {
                        // Other escapes (two characters): skip.
                        chars.next();
                        run_start = i + 2;
                        continue;
                    }
                    let end = seq[2..].find(|c: char| ('\x40'..='\x7e').contains(&c));
                    match end {
                        None => {
                            cut = Some(i);
                            break;
                        }
                        Some(e) => {
                            let params = &seq[2..2 + e];
                            let fin = seq.as_bytes()[2 + e];
                            if fin == b'm' {
                                self.style = apply_sgr(self.style, params);
                            }
                            let len = 2 + e + 1;
                            // Skip the sequence's characters.
                            while chars.peek().is_some_and(|(j, _)| *j < i + len) {
                                chars.next();
                            }
                            run_start = i + len;
                        }
                    }
                }
                _ => {}
            }
        }
        let end = cut.unwrap_or(rest.len());
        let style = self.style;
        self.partial.push(&rest[run_start..end], style);
        let mut pending = rest.as_bytes()[end..].to_vec();
        pending.extend_from_slice(&tail);
        self.pending = pending;
    }

    /// The question the run is waiting on, if its last output looks like
    /// one (a prompt is written without a newline).
    pub fn prompt(&self) -> Option<Prompt> {
        if !self.running() || self.partial.text.trim().is_empty() {
            return None;
        }
        // An options prompt wraps over lines; it starts at "➜ ".
        let mut text = self.partial.text.clone();
        for line in self.lines.iter().rev().take(6) {
            if text.contains('\u{279c}') {
                break;
            }
            text = format!("{} {text}", line.text);
        }
        Some(parse_prompt(&text))
    }
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params.split(';').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => style = Style::default(),
            1 => style.bold = true,
            4 => style.underline = true,
            22 => style.bold = false,
            24 => style.underline = false,
            c @ 30..=37 => style.color = Some(Color::Indexed((c - 30) as u8)),
            c @ 90..=97 => style.color = Some(Color::Indexed((c - 90 + 8) as u8)),
            39 => style.color = None,
            38 => match codes.get(i + 1) {
                Some(5) => {
                    style.color = codes.get(i + 2).map(|&n| Color::Indexed(n as u8));
                    i += 2;
                }
                Some(2) => {
                    if let (Some(&r), Some(&g), Some(&b)) =
                        (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4))
                    {
                        style.color = Some(Color::Rgb(r as u8, g as u8, b as u8));
                    }
                    i += 4;
                }
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    style
}

/// beets' prompts: "➜ [A]pply, More candidates, Skip, …, aBort?" (the key
/// is the capital letter; "[X]" is the default), "# selection (default
/// 1), …", or a question ending in a choice list: "(Y/n)", "(yes/no)",
/// "(Yes/no/select)" (a capitalised word is the default).
pub fn parse_prompt(text: &str) -> Prompt {
    let mut prompt = Prompt::default();
    if let Some(choices) = slash_choices(text) {
        prompt.choices = choices;
        return prompt;
    }
    if text.to_lowercase().contains("enter y or n") {
        prompt.choices = slash_choices("(y/n)").unwrap_or_default();
        return prompt;
    }
    if let Some(n) = number_range(text) {
        // replace's "Which song would you like to replace? [1-3] (0 to cancel)".
        prompt.numbers = n.min(9);
        if text.contains("0 to cancel") {
            prompt.choices.push(Choice {
                label: "Cancel".into(),
                answer: "0".into(),
                default: false,
            });
        }
        return prompt;
    }
    let Some(start) = text.rfind('\u{279c}') else {
        return prompt;
    };
    let body = text[start + '\u{279c}'.len_utf8()..].trim();
    let body = body.strip_suffix('?').unwrap_or(body);
    for part in body.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.starts_with("# selection") {
            prompt.numbers = 5;
            continue;
        }
        let (label, letter, default) = if let (Some(a), Some(b)) = (part.find('['), part.find(']'))
        {
            let letter = part[a + 1..b].to_string();
            (
                format!("{}{}{}", &part[..a], letter, &part[b + 1..]),
                letter,
                true,
            )
        } else {
            let letter = part
                .chars()
                .find(|c| c.is_uppercase())
                .or_else(|| part.chars().next())
                .map(String::from)
                .unwrap_or_default();
            (part.to_string(), letter, false)
        };
        // Present labels in sentence case ("aBort" → "Abort").
        let mut cs = label.chars();
        let label = match cs.next() {
            Some(f) => f.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase(),
            None => label,
        };
        prompt.choices.push(Choice {
            label,
            answer: letter.to_lowercase(),
            default,
        });
    }
    prompt
}

/// The highest number of a `[1-N]` menu prompt.
fn number_range(text: &str) -> Option<usize> {
    let start = text.rfind("[1-")?;
    let rest = &text[start + 3..];
    let end = rest.find(']')?;
    rest[..end].parse().ok().filter(|&n| n > 0)
}

/// The choices of a "(Yes/no/select)"-style list: the last parenthesised
/// group of two or more words separated by slashes.
fn slash_choices(text: &str) -> Option<Vec<Choice>> {
    let close = text.rfind(')')?;
    let open = text[..close].rfind('(')?;
    let words: Vec<&str> = text[open + 1..close].split('/').map(str::trim).collect();
    let valid = words.len() >= 2
        && words
            .iter()
            .all(|w| !w.is_empty() && w.chars().all(char::is_alphabetic));
    if !valid {
        return None;
    }
    // Several capitalised words ("Y/N") mark no default.
    let capitals = words
        .iter()
        .filter(|w| w.starts_with(char::is_uppercase))
        .count();
    Some(
        words
            .iter()
            .map(|w| {
                let mut cs = w.chars();
                let first = cs.next().unwrap_or('?');
                let label = match w.len() {
                    1 => match first.to_ascii_lowercase() {
                        'y' => "Yes".to_string(),
                        'n' => "No".to_string(),
                        _ => first.to_uppercase().to_string(),
                    },
                    _ => first.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase(),
                };
                Choice {
                    label,
                    answer: first.to_lowercase().to_string(),
                    default: capitals == 1 && first.is_uppercase(),
                }
            })
            .collect(),
    )
}

pub struct Runner {
    /// The program and the arguments before `ARGS…` (`beet-gui --beet` and
    /// the global options).
    program: Vec<OsString>,
    pub sessions: Vec<Session>,
    tx: flume::Sender<Event>,
    pub rx: flume::Receiver<Event>,
}

impl Runner {
    /// Runs `beet` as this executable with `--beet` and `globals`.
    pub fn new(globals: Vec<OsString>) -> Runner {
        // Bounded: a command that prints faster than the UI can draw is
        // slowed down (its pipe fills) instead of growing memory.
        let (tx, rx) = flume::bounded(EVENT_BACKLOG);
        Runner {
            program: std::iter::once(
                std::env::current_exe()
                    .unwrap_or_else(|_| "beet-gui".into())
                    .into_os_string(),
            )
            .chain(std::iter::once(BEET_FLAG.into()))
            .chain(globals)
            .collect(),
            sessions: vec![],
            tx,
            rx,
        }
    }

    /// Start `beet ARGS…`. Returns the session's index.
    pub fn start(&mut self, title: impl Into<String>, args: Vec<String>, mutates: bool) -> usize {
        let id = self.sessions.len();
        let mut session = Session {
            id,
            title: title.into(),
            args: args.clone(),
            lines: vec![],
            partial: Line::default(),
            status: Status::Running,
            started: Instant::now(),
            finished: None,
            mutates,
            child: None,
            stdin: None,
            style: Style::default(),
            pending: vec![],
        };
        session.push_line(Line {
            text: format!("$ beet {}", shell_join(&args)),
            styles: vec![(
                0..2,
                Style {
                    bold: true,
                    ..Style::default()
                },
            )],
        });
        let mut cmd = Command::new(&self.program[0]);
        cmd.args(&self.program[1..])
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Wide enough for beets' two-column change displays.
            .env("COLUMNS", "110");
        match cmd.spawn() {
            Ok(mut child) => {
                session.stdin = child.stdin.take();
                for stream in [
                    child
                        .stdout
                        .take()
                        .map(|s| Box::new(s) as Box<dyn Read + Send>),
                    child
                        .stderr
                        .take()
                        .map(|s| Box::new(s) as Box<dyn Read + Send>),
                ]
                .into_iter()
                .flatten()
                {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || pump(id, stream, tx));
                }
                let child = Arc::new(Mutex::new(child));
                session.child = Some(child.clone());
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let code = loop {
                        if let Ok(Some(status)) = lock(&child).try_wait() {
                            break status.code();
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    };
                    // Let the readers drain the pipes first.
                    std::thread::sleep(Duration::from_millis(60));
                    tx.send(Event::Exit { session: id, code }).ok();
                });
            }
            Err(e) => {
                session.status = Status::Failed(e.to_string());
                session.finished = Some(Instant::now());
            }
        }
        self.sessions.push(session);
        id
    }

    /// Apply one event. Returns the session if it finished.
    pub fn handle(&mut self, ev: Event) -> Option<usize> {
        match ev {
            Event::Output { session, bytes } => {
                self.sessions[session].feed(&bytes);
                None
            }
            Event::Exit { session, code } => {
                let s = &mut self.sessions[session];
                if !s.partial.text.is_empty() {
                    let line = std::mem::take(&mut s.partial);
                    s.push_line(line);
                }
                s.status = Status::Exited(code);
                s.finished = Some(Instant::now());
                s.stdin = None;
                s.child = None;
                Some(session)
            }
        }
    }

    /// Apply the events already waiting. Returns the sessions that finished.
    pub fn drain(&mut self) -> Vec<usize> {
        let mut finished = vec![];
        while let Ok(ev) = self.rx.try_recv() {
            finished.extend(self.handle(ev));
        }
        finished
    }
}

/// Lock a child process handle. A panic while it was held cannot leave a
/// `Child` inconsistent, so a poisoned lock is still usable.
fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn pump(session: usize, mut stream: Box<dyn Read + Send>, tx: flume::Sender<Event>) {
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx
                    .send(Event::Output {
                        session,
                        bytes: buf[..n].to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

/// Quote arguments for display, as a shell would need them.
pub fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_alphanumeric() || "-_./:=+,@%^".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            id: 0,
            title: String::new(),
            args: vec![],
            lines: vec![],
            partial: Line::default(),
            status: Status::Running,
            started: Instant::now(),
            finished: None,
            mutates: false,
            child: None,
            stdin: None,
            style: Style::default(),
            pending: vec![],
        }
    }

    #[test]
    fn splits_lines_and_colours() {
        let mut s = session();
        s.feed(b"plain\n\x1b[1;31mred\x1b[0m done\nprom");
        s.feed(b"pt? ");
        assert_eq!(s.lines.len(), 2);
        assert_eq!(s.lines[0].text, "plain");
        assert_eq!(s.lines[1].text, "red done");
        assert_eq!(s.lines[1].styles.len(), 1);
        assert_eq!(s.lines[1].styles[0].0, 0..3);
        assert!(s.lines[1].styles[0].1.bold);
        assert_eq!(s.lines[1].styles[0].1.color, Some(Color::Indexed(1)));
        assert_eq!(s.partial.text, "prompt? ");
    }

    #[test]
    fn escapes_and_utf8_split_across_reads() {
        let mut s = session();
        let bytes = "\x1b[32m\u{279c} ok\n".as_bytes();
        s.feed(&bytes[..3]);
        s.feed(&bytes[3..7]);
        s.feed(&bytes[7..]);
        assert_eq!(s.lines[0].text, "\u{279c} ok");
        assert_eq!(s.lines[0].styles[0].1.color, Some(Color::Indexed(2)));
    }

    #[test]
    fn carriage_return_rewinds() {
        let mut s = session();
        s.feed(b"10%\r20%\r\ndone\n");
        assert_eq!(s.lines[0].text, "20%");
        assert_eq!(s.lines[1].text, "done");
    }

    #[test]
    fn parses_option_prompts() {
        let p = parse_prompt(
            "\u{279c} [A]pply, More candidates, Skip, Use as-is, as Tracks, Group albums, Enter search, enter Id, aBort?",
        );
        let answers: Vec<_> = p.choices.iter().map(|c| c.answer.as_str()).collect();
        assert_eq!(answers, ["a", "m", "s", "u", "t", "g", "e", "i", "b"]);
        assert!(p.choices[0].default);
        assert_eq!(p.choices[0].label, "Apply");
        assert_eq!(p.choices[8].label, "Abort");
        assert_eq!(p.numbers, 0);

        let p = parse_prompt("\u{279c} # selection (default 1), Skip, Use as-is, aBort?");
        assert_eq!(p.numbers, 5);
        assert_eq!(p.choices.len(), 3);

        let p = parse_prompt("Really DELETE 3 files (yes/no)? ");
        assert_eq!(p.choices.len(), 2);
        let p = parse_prompt("Convert? (Y/n)");
        assert!(p.choices[0].default);

        let p = parse_prompt("Which song would you like to replace? [1-3] (0 to cancel): ");
        assert_eq!(p.numbers, 3);
        assert_eq!(p.choices.len(), 1);
        assert_eq!(p.choices[0].answer, "0");
        let p = parse_prompt("Are you sure you want to replace this track? (y/N): ");
        let got: Vec<_> = p
            .choices
            .iter()
            .map(|c| (c.answer.as_str(), c.default))
            .collect();
        assert_eq!(got, [("y", false), ("n", true)]);
    }

    #[test]
    fn parses_slash_choice_lists() {
        let p = parse_prompt("Really modify, move and write tags? (Yes/no/select)");
        let got: Vec<_> = p
            .choices
            .iter()
            .map(|c| (c.label.as_str(), c.answer.as_str(), c.default))
            .collect();
        assert_eq!(
            got,
            [
                ("Yes", "y", true),
                ("No", "n", false),
                ("Select", "s", false)
            ]
        );
        // Not a choice list.
        assert!(parse_prompt("Destination (optional)? ").choices.is_empty());
        assert_eq!(parse_prompt("➜ Enter Y or N:").choices.len(), 2);
    }

    /// The whole loop with a real child: output, a prompt, an answer on
    /// stdin, the exit code.
    #[test]
    fn answers_a_prompt_over_stdin() {
        let mut runner = Runner::new(vec![]);
        runner.program = vec![
            "sh".into(),
            "-c".into(),
            r#"echo "working"; printf 'Continue? (Y/n) '; read a; echo "got $a"; exit 3"#.into(),
            "sh".into(),
        ];
        let id = runner.start("test", vec![], false);
        let deadline = Instant::now() + Duration::from_secs(10);
        while runner.sessions[id].prompt().is_none() {
            assert!(Instant::now() < deadline, "no prompt");
            if let Ok(ev) = runner.rx.recv_timeout(Duration::from_millis(100)) {
                runner.handle(ev);
            }
        }
        let prompt = runner.sessions[id].prompt().unwrap();
        assert!(prompt.choices[0].default);
        runner.sessions[id].send("y");
        while runner.sessions[id].running() {
            assert!(Instant::now() < deadline, "did not exit");
            if let Ok(ev) = runner.rx.recv_timeout(Duration::from_millis(100)) {
                runner.handle(ev);
            }
        }
        let s = &runner.sessions[id];
        assert_eq!(s.status, Status::Exited(Some(3)));
        let text: Vec<_> = s.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, ["$ beet ", "working", "Continue? (Y/n) y", "got y"]);
    }

    #[test]
    fn quotes_for_display() {
        assert_eq!(
            shell_join(&["modify".into(), "id:3".into(), "title=A b".into()]),
            "modify id:3 'title=A b'"
        );
    }
}
