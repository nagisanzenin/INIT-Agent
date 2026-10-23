//! Text checks on the cached head files (docs/awareness.md §3.9, §8): the volatility lint
//! (T-AW-3) and the approval-language check (T-AW-15). The regexes of §8 are implemented by hand
//! (no regex crate). Std only and free of `crate::` paths, so `tests/self_md.rs` includes this file
//! by path.
//!
//! `\b` is ASCII: a word byte is `[A-Za-z0-9_]`; any other byte, including every non-ASCII byte,
//! is a non-word byte.

fn is_w(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `\b` at byte offset `i` (`i == s.len()` is allowed).
fn wb(s: &[u8], i: usize) -> bool {
    let before = i > 0 && i <= s.len() && is_w(s[i - 1]);
    let after = i < s.len() && is_w(s[i]);
    before != after
}

fn digits(s: &[u8], i: usize, n: usize) -> bool {
    i + n <= s.len() && s[i..i + n].iter().all(u8::is_ascii_digit)
}

fn byte(s: &[u8], i: usize, b: u8) -> bool {
    s.get(i) == Some(&b)
}

/// Matches of the volatility lint (awareness.md T-AW-3) as `(rule, byte offset, matched text)`:
/// `\b20\d\d-\d\d-\d\d\b` (date), `\b\d\d:\d\d(:\d\d)?\b` (time), `\b[0-9a-f]{7,}\b` (hex),
/// `\$\d` (money), `\b\d{8}T\d{6}Z\b` (stamp), and with `levels` also `\bL[0-4]\b` (level, for
/// GENESIS.md and SELF.md only: the constitution names the levels by design).
pub fn volatility_hits(text: &str, levels: bool) -> Vec<(&'static str, usize, String)> {
    let s = text.as_bytes();
    let mut hits = Vec::new();
    let mut hit = |rule: &'static str, i: usize, end: usize| {
        hits.push((rule, i, String::from_utf8_lossy(&s[i..end]).into_owned()));
    };
    for i in 0..s.len() {
        if s[i] == b'$' && digits(s, i + 1, 1) {
            hit("money", i, i + 2);
        }
        if !wb(s, i) || !is_w(s[i]) {
            continue;
        }
        if byte(s, i, b'2')
            && byte(s, i + 1, b'0')
            && digits(s, i + 2, 2)
            && byte(s, i + 4, b'-')
            && digits(s, i + 5, 2)
            && byte(s, i + 7, b'-')
            && digits(s, i + 8, 2)
            && wb(s, i + 10)
        {
            hit("date", i, i + 10);
        }
        if digits(s, i, 2) && byte(s, i + 2, b':') && digits(s, i + 3, 2) {
            if byte(s, i + 5, b':') && digits(s, i + 6, 2) && wb(s, i + 8) {
                hit("time", i, i + 8);
            } else if wb(s, i + 5) {
                hit("time", i, i + 5);
            }
        }
        if digits(s, i, 8)
            && byte(s, i + 8, b'T')
            && digits(s, i + 9, 6)
            && byte(s, i + 15, b'Z')
            && wb(s, i + 16)
        {
            hit("stamp", i, i + 16);
        }
        // The word starting here (all its bytes are word bytes, so the two remaining rules
        // match only a whole word).
        let end = (i..s.len()).find(|&j| !is_w(s[j])).unwrap_or(s.len());
        let word = &s[i..end];
        if word.len() >= 7
            && word
                .iter()
                .all(|&b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            hit("hex", i, end);
        }
        if levels && word.len() == 2 && word[0] == b'L' && (b'0'..=b'4').contains(&word[1]) {
            hit("level", i, end);
        }
    }
    hits
}

/// Approval language (awareness.md T-AW-15): the phrases "ask your owner", "owner approves",
/// "needs approval" and "get approval" in any case and across line breaks, and the all-caps
/// words `CRITICAL` and `NEVER`.
pub fn approval_hits(text: &str) -> Vec<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_ascii_lowercase();
    let mut hits: Vec<String> = [
        "ask your owner",
        "owner approves",
        "needs approval",
        "get approval",
    ]
    .iter()
    .filter(|p| lower.contains(*p))
    .map(|p| p.to_string())
    .collect();
    let s = flat.as_bytes();
    for word in ["CRITICAL", "NEVER"] {
        let w = word.as_bytes();
        let found = (0..s.len().saturating_sub(w.len() - 1))
            .any(|i| s[i..].starts_with(w) && wb(s, i) && wb(s, i + w.len()));
        if found {
            hits.push(word.to_string());
        }
    }
    hits
}

/// The `---` front matter of a skill file (the lines between the first two `---` lines), or ""
/// when there is none.
pub fn front_matter(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return String::new();
    }
    lines
        .take_while(|l| l.trim_end() != "---")
        .collect::<Vec<_>>()
        .join("\n")
}
