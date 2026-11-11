//! Terminal helpers: colors and beets-style prompts.

use std::io::{BufRead, IsTerminal, Write};

pub fn color_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

pub fn colorize(code: &str, s: &str) -> String {
    if color_enabled() && !s.is_empty() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn green(s: &str) -> String {
    colorize("32", s)
}
pub fn yellow(s: &str) -> String {
    colorize("33", s)
}
pub fn red(s: &str) -> String {
    colorize("31", s)
}
pub fn bold(s: &str) -> String {
    colorize("1", s)
}
pub fn dim(s: &str) -> String {
    colorize("2", s)
}

/// "Similarity: 97.3%" colored by match quality.
pub fn similarity(dist: f64, strong: f64, medium: f64) -> String {
    let s = format!("{:.1}%", (1.0 - dist) * 100.0);
    if dist <= strong {
        green(&s)
    } else if dist <= medium {
        yellow(&s)
    } else {
        red(&s)
    }
}

pub fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Letter(char),
    Number(usize),
}

/// Show options like "Apply", "More candidates", "aBort": the key is the first
/// uppercase letter (or the first letter). With `numbers`, also accept 1..=n.
/// `default` is returned on empty input. Returns None at EOF.
pub fn options(opts: &[&str], numbers: Option<usize>, default: Option<Choice>) -> Option<Choice> {
    let keys: Vec<char> = opts
        .iter()
        .map(|o| {
            o.chars()
                .find(|c| c.is_uppercase())
                .unwrap_or_else(|| o.chars().next().unwrap())
                .to_ascii_lowercase()
        })
        .collect();
    let shown: Vec<String> = opts
        .iter()
        .zip(&keys)
        .map(|(o, k)| {
            let pos = o
                .char_indices()
                .find(|(_, c)| c.to_ascii_lowercase() == *k)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let (a, b) = o.split_at(pos);
            let mut rest = b.chars();
            let key = rest.next().unwrap();
            format!(
                "{a}{}{}",
                bold(&format!("[{}]", key.to_ascii_uppercase())),
                rest.as_str()
            )
        })
        .collect();
    let mut prompt = String::new();
    if let Some(n) = numbers {
        let d = match &default {
            Some(Choice::Number(d)) => format!(" (default {d})"),
            _ => String::new(),
        };
        prompt.push_str(&format!("# selection 1-{n}{d}, "));
    }
    prompt.push_str(&shown.join(", "));
    prompt.push_str("? ");
    loop {
        let line = read_line(&prompt)?;
        if line.is_empty() {
            if let Some(d) = default {
                return Some(d);
            }
            continue;
        }
        if let (Some(n), Ok(i)) = (numbers, line.parse::<usize>())
            && (1..=n).contains(&i)
        {
            return Some(Choice::Number(i));
        }
        let c = line.chars().next().unwrap().to_ascii_lowercase();
        if keys.contains(&c) && line.chars().count() == 1 {
            return Some(Choice::Letter(c));
        }
        println!("Please enter one of the options.");
    }
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
    let mut line = read_line(&format!("{prompt} "))?;
    loop {
        let answer = line.trim().to_lowercase();
        match answer.chars().next() {
            None if !require => return letters.first().copied(),
            Some(c) if letters.contains(&c) => return Some(c),
            _ => line = read_line(&format!("{fallback} "))?,
        }
    }
}

/// beets' `input_select_objects`: ask "PROMPT? (Yes/no/select)" and return
/// all, none, or (select) the objects confirmed one by one, after showing
/// each with `show`.
pub fn select_objects<T: Clone>(prompt: &str, objs: &[T], mut show: impl FnMut(&T)) -> Vec<T> {
    let choice = input_options(
        &['y', 'n', 's'],
        false,
        &format!("{prompt}? (Yes/no/select)"),
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
