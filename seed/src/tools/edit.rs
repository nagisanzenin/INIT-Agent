//! `edit` (tools.md §3.4): exact replacement with two normalized fallbacks, uniqueness and
//! overlap checks against the original file, and a unified diff built from the matched ranges.

use std::path::PathBuf;

use serde_json::{Map, Value};

use super::args::{check_fields, req, req_str, type_error};
use super::write::{atomic_write, write_target};
use super::{
    Limits, PathCx, Ran, ToolCx, ToolError, commas, io_error, is_directory, resolve, text_result,
};

#[derive(Clone, Debug)]
pub struct EditPair {
    pub old_text: String,
    pub new_text: String,
}

#[derive(Clone, Debug)]
pub struct EditArgs {
    pub path: String,
    pub abs: PathBuf,
    pub edits: Vec<EditPair>,
}

pub fn parse(m: &Map<String, Value>, cx: &PathCx, lim: &Limits) -> Result<EditArgs, ToolError> {
    check_fields(m, &["edits", "path"], "")?;
    let path = req_str(m, "path", "")?;
    let arr = match req(m, "edits", "")? {
        Value::Array(a) => a,
        other => return Err(type_error("edits", "an array", other)),
    };
    if arr.is_empty() {
        return Err(ToolError::invalid("edits: at least one edit is required"));
    }
    if arr.len() > lim.edit_count {
        return Err(ToolError::invalid(format!(
            "edits: at most {} edits per call (got {}); split them over several calls",
            lim.edit_count,
            arr.len()
        )));
    }
    let mut edits = Vec::with_capacity(arr.len());
    for (i, e) in arr.iter().enumerate() {
        let p = format!("edits[{i}]");
        let Value::Object(o) = e else {
            return Err(type_error(&p, "an object", e));
        };
        check_fields(o, &["new_text", "old_text"], &p)?;
        edits.push(EditPair {
            old_text: req_str(o, "old_text", &p)?,
            new_text: req_str(o, "new_text", &p)?,
        });
    }
    let abs = resolve(&path, cx)?;
    Ok(EditArgs { path, abs, edits })
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

/// Codex `seek_sequence::normalise` / Pi `normalizeForFuzzyMatch` punctuation folding.
pub fn map_punct(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}'..='\u{201B}' => '\'',
            '\u{201C}'..='\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            c => c,
        })
        .collect()
}

fn lead(s: &str) -> &str {
    let n = s.len() - s.trim_start_matches([' ', '\t']).len();
    &s[..n]
}

/// Line table of the (LF-normalized) text: `(start, end)` with `end` excluding `\n`.
struct Lines {
    spans: Vec<(usize, usize)>,
    len: usize,
}

impl Lines {
    fn new(text: &str) -> Lines {
        let mut spans = Vec::new();
        let mut s = 0;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                spans.push((s, i));
                s = i + 1;
            }
        }
        if s < text.len() {
            spans.push((s, text.len()));
        }
        Lines {
            spans,
            len: text.len(),
        }
    }
    /// 0-based line index containing byte `off`.
    fn line_of(&self, off: usize) -> usize {
        match self.spans.binary_search_by(|&(s, _)| s.cmp(&off)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }
    fn has_nl(&self, i: usize) -> bool {
        self.spans[i].1 < self.len
    }
    /// End of line `i` including its `\n`.
    fn end_nl(&self, i: usize) -> usize {
        self.spans[i].1 + self.has_nl(i) as usize
    }
}

#[derive(Clone, Debug)]
enum Reindent {
    None,
    Add(String),
    Strip(String),
}

#[derive(Clone, Debug)]
struct Hit {
    start: usize,
    end: usize,
    tier: u8,
    reindent: Reindent,
}

enum Found {
    One(Hit),
    Many(Vec<usize>),
    None,
}

struct Norm {
    n2: Vec<String>,
    n3: Vec<String>,
}

fn find(text: &str, lines: &Lines, norm: &mut Option<Norm>, o: &str) -> Found {
    // Tier 1: exact, overlapping occurrences counted.
    let mut hits = Vec::new();
    let mut pos = 0;
    while let Some(i) = text[pos..].find(o) {
        let h = pos + i;
        hits.push(h);
        pos = h + text[h..].chars().next().map(char::len_utf8).unwrap_or(1);
        if pos > text.len() {
            break;
        }
    }
    match hits.len() {
        0 => {}
        1 => {
            return Found::One(Hit {
                start: hits[0],
                end: hits[0] + o.len(),
                tier: 1,
                reindent: Reindent::None,
            });
        }
        _ => return Found::Many(hits),
    }
    let nm = norm.get_or_insert_with(|| Norm {
        n2: lines
            .spans
            .iter()
            .map(|&(s, e)| map_punct(&text[s..e]).trim_end().to_string())
            .collect(),
        n3: lines
            .spans
            .iter()
            .map(|&(s, e)| map_punct(&text[s..e]).trim().to_string())
            .collect(),
    });
    let (o_body, o_nl) = match o.strip_suffix('\n') {
        Some(b) => (b, true),
        None => (o, false),
    };
    let o_lines: Vec<&str> = o_body.split('\n').collect();
    let k = o_lines.len();
    let n = lines.spans.len();
    if k == 0 || k > n {
        return Found::None;
    }
    let range = |s: usize| {
        let last = s + k - 1;
        let end = lines.spans[last].1 + (o_nl && lines.has_nl(last)) as usize;
        (lines.spans[s].0, end)
    };
    // Tier 2: trailing whitespace and punctuation variants.
    let p2: Vec<String> = o_lines
        .iter()
        .map(|l| map_punct(l).trim_end().to_string())
        .collect();
    let w2: Vec<usize> = (0..=n - k)
        .filter(|&s| (0..k).all(|j| nm.n2[s + j] == p2[j]))
        .collect();
    match w2.len() {
        0 => {}
        1 => {
            let (start, end) = range(w2[0]);
            return Found::One(Hit {
                start,
                end,
                tier: 2,
                reindent: Reindent::None,
            });
        }
        _ => return Found::Many(w2.iter().map(|&s| lines.spans[s].0).collect()),
    }
    // Tier 3: also a uniform indentation difference.
    let p3: Vec<String> = o_lines
        .iter()
        .map(|l| map_punct(l).trim().to_string())
        .collect();
    let mut w3: Vec<(usize, Reindent)> = Vec::new();
    for s in 0..=n - k {
        if !(0..k).all(|j| nm.n3[s + j] == p3[j]) {
            continue;
        }
        if let Some(r) = indent_delta(text, lines, s, &o_lines) {
            w3.push((s, r));
        }
    }
    match w3.len() {
        0 => Found::None,
        1 => {
            let (s, r) = w3.pop().unwrap();
            let (start, end) = range(s);
            Found::One(Hit {
                start,
                end,
                tier: 3,
                reindent: r,
            })
        }
        _ => Found::Many(w3.iter().map(|(s, _)| lines.spans[*s].0).collect()),
    }
}

/// One fixed `D` with `lead(f) == D + lead(p)` (add D), or one fixed `E` with
/// `lead(p) == E + lead(f)` (strip E), over every non-blank line pair.
fn indent_delta(text: &str, lines: &Lines, s: usize, o_lines: &[&str]) -> Option<Reindent> {
    let mut d: Option<Option<String>> = None; // None = undecided; Some(None) = mode failed
    let mut e: Option<Option<String>> = None;
    for (j, p) in o_lines.iter().enumerate() {
        let (fs, fe) = lines.spans[s + j];
        let f = &text[fs..fe];
        if f.trim().is_empty() && p.trim().is_empty() {
            continue;
        }
        let (lf, lp) = (lead(f), lead(p));
        let dj = lf
            .strip_suffix(lp)
            .filter(|_| lf.ends_with(lp))
            .map(str::to_string);
        let ej = lp
            .strip_suffix(lf)
            .filter(|_| lp.ends_with(lf))
            .map(str::to_string);
        // A fixed string must be a prefix: lead(f) = D + lead(p) means lf ends with lp.
        d = match (d, dj) {
            (None, x) => Some(x),
            (Some(Some(prev)), Some(x)) if prev == x => Some(Some(prev)),
            _ => Some(None),
        };
        e = match (e, ej) {
            (None, x) => Some(x),
            (Some(Some(prev)), Some(x)) if prev == x => Some(Some(prev)),
            _ => Some(None),
        };
    }
    match (d.flatten(), e.flatten()) {
        (Some(dd), _) if !dd.is_empty() => Some(Reindent::Add(dd)),
        (_, Some(ee)) if !ee.is_empty() => Some(Reindent::Strip(ee)),
        (Some(_), _) | (_, Some(_)) => Some(Reindent::None),
        (None, None) => None,
    }
}

fn apply_reindent(n: &str, r: &Reindent) -> String {
    match r {
        Reindent::None => n.to_string(),
        Reindent::Add(d) => map_nonblank(n, |l| format!("{d}{l}")),
        Reindent::Strip(e) => {
            map_nonblank(n, |l| l.strip_prefix(e.as_str()).unwrap_or(l).to_string())
        }
    }
}

fn map_nonblank(n: &str, f: impl Fn(&str) -> String) -> String {
    n.split('\n')
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else {
                f(l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe_indent(s: &str) -> String {
    let n = s.chars().count();
    if s.chars().all(|c| c == ' ') {
        format!("{n} space{}", if n == 1 { "" } else { "s" })
    } else if s.chars().all(|c| c == '\t') {
        format!("{n} tab{}", if n == 1 { "" } else { "s" })
    } else {
        format!("{s:?}")
    }
}

fn line_list(lines: &Lines, starts: &[usize]) -> String {
    starts
        .iter()
        .take(5)
        .map(|&s| (lines.line_of(s) + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------------------------

struct Applied {
    idx: usize,
    hit: Hit,
    repl: String,
}

pub fn run(a: &EditArgs, cx: &ToolCx, lim: &Limits) -> Result<Ran, ToolError> {
    let md = match std::fs::metadata(&a.abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                ToolError::new("not_found", format!("{} does not exist", a.path))
                    .hint("use write to create a new file"),
            );
        }
        Err(e) => return Err(io_error(&a.path, &e)),
    };
    if md.is_dir() {
        return Err(is_directory(&a.path));
    }
    if md.len() > lim.edit_file_bytes {
        return Err(ToolError::new(
            "too_large",
            format!(
                "file is {} bytes; edit handles at most {} MiB",
                commas(md.len()),
                lim.edit_file_bytes >> 20
            ),
        ));
    }
    let bytes = std::fs::read(&a.abs).map_err(|e| io_error(&a.path, &e))?;
    let Ok(raw) = String::from_utf8(bytes) else {
        return Err(ToolError::new(
            "binary_file",
            format!(
                "{} is not valid UTF-8; edit needs a UTF-8 text file",
                a.path
            ),
        ));
    };
    let (bom, body) = match raw.strip_prefix('\u{FEFF}') {
        Some(b) => (true, b),
        None => (false, raw.as_str()),
    };
    let crlf = body
        .find('\n')
        .map(|i| i > 0 && body.as_bytes()[i - 1] == b'\r')
        .unwrap_or(false);
    let text = body.replace("\r\n", "\n");
    let lines = Lines::new(&text);
    let mut norm = None;
    let mut applied: Vec<Applied> = Vec::with_capacity(a.edits.len());
    for (i, e) in a.edits.iter().enumerate() {
        let o = e.old_text.replace("\r\n", "\n");
        let n = e.new_text.replace("\r\n", "\n");
        if o.is_empty() {
            return Err(ToolError::invalid(format!("edits[{i}].old_text is empty")));
        }
        match find(&text, &lines, &mut norm, &o) {
            Found::One(hit) => {
                let repl = if hit.tier == 3 {
                    apply_reindent(&n, &hit.reindent)
                } else {
                    n
                };
                applied.push(Applied { idx: i, hit, repl });
            }
            Found::Many(starts) => {
                return Err(ToolError::new(
                    "ambiguous_match",
                    format!("edits[{i}].old_text matches {} places in {}", starts.len(), a.path),
                )
                .hint(format!(
                    "found {} matches (lines {}); add surrounding lines to old_text so it matches one place.",
                    starts.len(),
                    line_list(&lines, &starts)
                )));
            }
            Found::None => {
                return Err(ToolError::new(
                    "no_match",
                    format!("edits[{i}].old_text was not found in {}", a.path),
                )
                .hint(no_match_hint(&text, &lines, &o)));
            }
        }
    }
    applied.sort_by_key(|x| x.hit.start);
    for w in applied.windows(2) {
        let (p, q) = (&w[0], &w[1]);
        if q.hit.start < p.hit.end {
            let (x, y) = if p.idx < q.idx { (p, q) } else { (q, p) };
            let span = |h: &Hit| {
                format!(
                    "lines {}-{}",
                    lines.line_of(h.start) + 1,
                    lines.line_of(h.end.saturating_sub(1).max(h.start)) + 1
                )
            };
            return Err(ToolError::new(
                "overlapping_edits",
                format!("edits[{}] and edits[{}] overlap", x.idx, y.idx),
            )
            .hint(format!(
                "edits[{}] ({}) and edits[{}] ({}) overlap; merge them into one edit.",
                x.idx,
                span(&x.hit),
                y.idx,
                span(&y.hit)
            )));
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for ap in &applied {
        out.push_str(&text[pos..ap.hit.start]);
        out.push_str(&ap.repl);
        pos = ap.hit.end;
    }
    out.push_str(&text[pos..]);
    if out == text {
        return Err(ToolError::new(
            "no_change",
            format!("the edits leave {} unchanged", a.path),
        ));
    }
    let diff = diff(&text, &lines, &applied, &a.path, lim.diff_lines);
    let mut disk = if crlf { out.replace('\n', "\r\n") } else { out };
    if bom {
        disk.insert(0, '\u{FEFF}');
    }
    atomic_write(&write_target(&a.abs), disk.as_bytes()).map_err(|e| io_error(&a.path, &e))?;
    let k = applied.len();
    let mut notes = Vec::new();
    let mut by_idx: Vec<&Applied> = applied.iter().collect();
    by_idx.sort_by_key(|x| x.idx);
    for ap in by_idx {
        match (ap.hit.tier, &ap.hit.reindent) {
            (2, _) => notes.push(format!(
                "edits[{}] matched ignoring trailing whitespace and punctuation variants",
                ap.idx
            )),
            (3, Reindent::Add(d)) => notes.push(format!(
                "edits[{}] matched ignoring indentation (re-indented by {})",
                ap.idx,
                describe_indent(d)
            )),
            (3, Reindent::Strip(e)) => notes.push(format!(
                "edits[{}] matched ignoring indentation (indentation reduced by {})",
                ap.idx,
                describe_indent(e)
            )),
            (3, Reindent::None) => {
                notes.push(format!("edits[{}] matched ignoring indentation", ap.idx))
            }
            _ => {}
        }
    }
    let mut head = format!(
        "Edited {}: {k} replacement{}",
        a.path,
        if k == 1 { "" } else { "s" }
    );
    for n in &notes {
        head.push_str("; ");
        head.push_str(n);
    }
    head.push('.');
    Ok(Ran {
        result: text_result(cx.call_id, format!("{head}\n{diff}")),
        summary: format!("{k} replacement{}", if k == 1 { "" } else { "s" }),
    })
}

fn no_match_hint(text: &str, lines: &Lines, o: &str) -> String {
    let first = o.split('\n').map(str::trim).find(|l| !l.is_empty());
    if let Some(first) = first {
        let at: Vec<String> = lines
            .spans
            .iter()
            .enumerate()
            .filter(|&(_, &(s, e))| text[s..e].trim() == first)
            .take(5)
            .map(|(i, _)| (i + 1).to_string())
            .collect();
        if !at.is_empty() {
            return format!(
                "the first line of old_text appears (ignoring indentation) at line {}; the lines after it differ. Re-read that region and copy the text exactly.",
                at.join(", ")
            );
        }
    }
    "no line of the file matches the first line of old_text; the file may have changed since you read it. Re-read it.".to_string()
}

fn split_lines(s: &str) -> Vec<&str> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    if s.is_empty() {
        return vec![""];
    }
    s.split('\n').collect()
}

struct Change<'a> {
    a: usize,
    old: Vec<&'a str>,
    new: Vec<String>,
}

const CTX: usize = 2;

/// Unified diff with 2 context lines, straight from the matched ranges (§3.4).
fn diff(text: &str, lines: &Lines, applied: &[Applied], path: &str, cap: usize) -> String {
    let mut changes: Vec<Change> = Vec::new();
    for ap in applied {
        let a = lines.line_of(ap.hit.start);
        let b = lines.line_of(ap.hit.end.saturating_sub(1).max(ap.hit.start));
        let (bs, be) = (lines.spans[a].0, lines.end_nl(b));
        let old = split_lines(&text[bs..be]);
        let new_block = format!(
            "{}{}{}",
            &text[bs..ap.hit.start],
            ap.repl,
            &text[ap.hit.end..be]
        );
        let new: Vec<String> = if new_block.is_empty() {
            Vec::new()
        } else {
            split_lines(&new_block)
                .into_iter()
                .map(str::to_string)
                .collect()
        };
        changes.push(Change { a, old, new });
    }
    let all: Vec<&str> = lines.spans.iter().map(|&(s, e)| &text[s..e]).collect();
    let mut body: Vec<String> = Vec::new();
    let mut delta: i64 = 0;
    let mut i = 0;
    while i < changes.len() {
        let mut j = i;
        while j + 1 < changes.len() {
            let end = changes[j].a + changes[j].old.len();
            if changes[j + 1].a.saturating_sub(end) < 2 * CTX {
                j += 1;
            } else {
                break;
            }
        }
        let first = &changes[i];
        let last = &changes[j];
        let c0 = first.a.saturating_sub(CTX);
        let c1 = (last.a + last.old.len() + CTX).min(all.len());
        let mut hunk = Vec::new();
        let mut cur = c0;
        let (mut olds, mut news) = (0usize, 0usize);
        for ch in &changes[i..=j] {
            while cur < ch.a {
                hunk.push(format!(" {}", all[cur]));
                cur += 1;
            }
            for l in &ch.old {
                hunk.push(format!("-{l}"));
            }
            for l in &ch.new {
                hunk.push(format!("+{l}"));
            }
            olds += ch.old.len();
            news += ch.new.len();
            cur = ch.a + ch.old.len();
        }
        while cur < c1 {
            hunk.push(format!(" {}", all[cur]));
            cur += 1;
        }
        let old_count = c1 - c0;
        let new_count = old_count - olds + news;
        let new_start = (c0 as i64 + 1 + delta).max(1);
        body.push(format!(
            "@@ -{},{} +{},{} @@",
            c0 + 1,
            old_count,
            new_start,
            new_count
        ));
        body.extend(hunk);
        delta += news as i64 - olds as i64;
        i = j + 1;
    }
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let shown = body.len().min(cap);
    for l in &body[..shown] {
        out.push_str(l);
        out.push('\n');
    }
    if body.len() > cap {
        out.push_str(&format!(
            "[diff truncated: {} more lines; read the file to check the result]\n",
            body.len() - cap
        ));
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}
