//! Change highlighting as in beets' `beets.util.diff`: Python difflib's
//! `SequenceMatcher` opcodes over characters, and `colordiff`.

use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal,
    Replace,
    Delete,
    Insert,
}

/// difflib's `find_longest_match` with no junk: the longest common block
/// in `a[alo..ahi]` and `b[blo..bhi]`, earliest in `a` (then `b`) on ties.
fn longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
    // j2len[j] = length of the match ending at a[i-1], b[j].
    let mut j2len = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut new = vec![0usize; b.len() + 1];
        for (j, bc) in b.iter().enumerate().take(bhi).skip(blo) {
            if bc != ac {
                continue;
            }
            let k = if j > 0 { j2len[j - 1] } else { 0 } + 1;
            new[j] = k;
            if k > bestsize {
                besti = i + 1 - k;
                bestj = j + 1 - k;
                bestsize = k;
            }
        }
        j2len = new;
    }
    (besti, bestj, bestsize)
}

/// difflib's `get_matching_blocks`, adjacent blocks merged.
fn matching_blocks(a: &[char], b: &[char]) -> Vec<(usize, usize, usize)> {
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = vec![];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, b, alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    let mut merged: Vec<(usize, usize, usize)> = vec![];
    for (i, j, k) in blocks {
        match merged.last_mut() {
            Some((i1, j1, k1)) if *i1 + *k1 == i && *j1 + *k1 == j => *k1 += k,
            _ => merged.push((i, j, k)),
        }
    }
    merged.push((a.len(), b.len(), 0));
    merged
}

/// difflib's `get_opcodes`: (op, a_start, a_end, b_start, b_end).
pub fn opcodes(a: &[char], b: &[char]) -> Vec<(Op, usize, usize, usize, usize)> {
    let (mut i, mut j) = (0, 0);
    let mut out = vec![];
    for (ai, bj, size) in matching_blocks(a, b) {
        let op = match (i < ai, j < bj) {
            (true, true) => Some(Op::Replace),
            (true, false) => Some(Op::Delete),
            (false, true) => Some(Op::Insert),
            _ => None,
        };
        if let Some(op) = op {
            out.push((op, i, ai, j, bj));
        }
        i = ai + size;
        j = bj + size;
        if size > 0 {
            out.push((Op::Equal, ai, i, bj, j));
        }
    }
    out
}

/// beets' `text_diff_removed` / `text_diff_added` colours.
pub fn removed(s: &str) -> String {
    ui::colorize("1;31", s)
}
pub fn added(s: &str) -> String {
    ui::colorize("1;32", s)
}

/// beets' `colordiff`: highlight what changed between two strings.
pub fn colordiff(a: &str, b: &str) -> (String, String) {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (mut before, mut after) = (String::new(), String::new());
    for (op, a0, a1, b0, b1) in opcodes(&ac, &bc) {
        let ap: String = ac[a0..a1].iter().collect();
        let bp: String = bc[b0..b1].iter().collect();
        match op {
            Op::Equal => {
                before += &ap;
                after += &bp;
            }
            Op::Delete => before += &removed(&ap),
            Op::Insert => after += &added(&bp),
            Op::Replace => {
                before += &removed(&ap);
                after += &added(&bp);
            }
        }
    }
    (before, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(a: &str, b: &str) -> Vec<(Op, usize, usize, usize, usize)> {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        opcodes(&a, &b)
    }

    #[test]
    fn opcodes_match_python_difflib() {
        // difflib.SequenceMatcher(None, "qabxcd", "abycdf").get_opcodes()
        use Op::*;
        assert_eq!(
            ops("qabxcd", "abycdf"),
            [
                (Delete, 0, 1, 0, 0),
                (Equal, 1, 3, 0, 2),
                (Replace, 3, 4, 2, 3),
                (Equal, 4, 6, 3, 5),
                (Insert, 6, 6, 5, 6),
            ]
        );
        assert_eq!(ops("", "ab"), [(Insert, 0, 0, 0, 2)]);
        assert_eq!(ops("ab", "ab"), [(Equal, 0, 2, 0, 2)]);
        assert!(ops("", "").is_empty());
        assert_eq!(
            ops("abcabc", "cbacba"),
            [
                (Insert, 0, 0, 0, 2),
                (Equal, 0, 1, 2, 3),
                (Insert, 1, 1, 3, 4),
                (Equal, 1, 2, 4, 5),
                (Delete, 2, 3, 5, 5),
                (Equal, 3, 4, 5, 6),
                (Delete, 4, 6, 6, 6),
            ]
        );
        // ("Hello", "Help"): equal "Hel", replace "lo" -> "p".
        assert_eq!(
            ops("Hello", "Help"),
            [(Equal, 0, 3, 0, 3), (Replace, 3, 5, 3, 4)]
        );
    }

    #[test]
    fn colordiff_without_colour_returns_the_strings() {
        assert_eq!(
            colordiff("Title", "Title (live)"),
            ("Title".into(), "Title (live)".into())
        );
    }
}
