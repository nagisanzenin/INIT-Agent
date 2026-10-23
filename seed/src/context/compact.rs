//! Compaction pieces (log-context.md §3.12): the cut point, the summary prompt, the summarizer seam,
//! parsing and id validation of the anchored summary, deterministic repair, and the body-generated
//! sections (owner messages, artifact index). The epoch itself runs in `engine.rs`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{ARTIFACT_ROWS, OWNER_MSG_CHARS, OWNER_MSGS_KEPT};
use super::project::{Loaded, Msg};
use crate::log::event::{EventBody, Kind, UsageEv, joined_text};

// ---------------------------------------------------------------------------------------------
// Cut point (§3.12.2)
// ---------------------------------------------------------------------------------------------

/// A cut may sit only before an assistant message, an owner message or a boot event, so a
/// `tool_use` and its results always stay on the same side.
pub fn is_boundary(l: &Loaded) -> bool {
    match &l.ev.body {
        EventBody::Message(m) => m.is_assistant() || m.is_owner(),
        EventBody::Boot(_) => true,
        _ => false,
    }
}

pub fn tail_tokens(tokens: &[u64], i: usize) -> u64 {
    tokens[i..].iter().sum()
}

/// Index of the first kept event, or `None` when nothing can be summarized. `tokens[i]` is the
/// (masked) estimate of `window[i]`.
pub fn choose_cut(window: &[Loaded], tokens: &[u64], keep: u64, max_tail: u64) -> Option<usize> {
    let mut tail: u64 = 0;
    let mut best_over: Option<usize> = None;
    let mut last_boundary: Option<usize> = None;
    for i in (1..window.len()).rev() {
        tail += tokens[i];
        if is_boundary(&window[i]) {
            if last_boundary.is_none() {
                last_boundary = Some(i);
            }
            if tail >= keep {
                best_over = Some(i);
                break;
            }
        }
    }
    match best_over {
        Some(i) if tail_tokens(tokens, i) <= max_tail => Some(i),
        Some(_) => last_boundary, // one huge step: keep only the last step
        None => None,             // whole window < keep: compaction cannot help
    }
}

// ---------------------------------------------------------------------------------------------
// Prompt and the summarizer seam (§3.12.3, §4.4)
// ---------------------------------------------------------------------------------------------

pub const SUMMARY_PROMPT: &str =
    "[init:compact] Your context is about to be compacted. Write the updated session summary now.
Respond with text only, in exactly the format below. Do not call any tools.

If an [init:summary] block appears earlier, it is the previous summary: merge into it everything
that happened after it. Do not rewrite it from scratch.
- Keep every existing item with its id. You may reword an item, move it between Progress
  subsections, or merge it into another item and say so in that item.
- To drop an item, put its id on the \"Retired:\" line.
- Give a new item the next unused number for its section letter (I, C, P, D, E, N, K).
- A section with nothing in it keeps its heading and the line \"- none\".

Preserve exactly: problems hit and how each was handled; approaches tried or set aside, and why;
every decision, constraint and preference, as stated; where the work stands now; what is still
open, promised or expected next; and names, numbers, paths, commands, event ids and exact wording
that would be hard to reconstruct. Condense your own reasoning to what it concluded or produced.
Do not list owner messages or files; the harness adds those.
{focus}
<summary>
## Intent
## Constraints and preferences
## Progress
### Done
### In progress
### Blocked
## Decisions
## Open errors
## Next steps
## Critical details
Retired:
</summary>";

pub const SOFT_CAP_LINE: &str =
    "The summary is over its budget: shorten item texts and retire finished items; keep ids.";
pub const RETRY_FORMAT_TEXT: &str = "[init:compact] Text only, in the required format.";

/// The prompt with `{focus}` replaced: empty, or `The owner asked this summary to focus on: <focus>`,
/// followed by the soft-cap line when the previous summary was over budget.
pub fn summary_prompt(focus: Option<&str>, over_soft_cap: bool) -> String {
    let mut lines = Vec::new();
    if let Some(f) = focus.filter(|f| !f.trim().is_empty()) {
        lines.push(format!(
            "The owner asked this summary to focus on: {}",
            f.trim()
        ));
    }
    if over_soft_cap {
        lines.push(SOFT_CAP_LINE.to_string());
    }
    SUMMARY_PROMPT.replace("{focus}", &lines.join("\n"))
}

/// The fork request: exactly the current request plus one user-side item (the prompt), and on a retry
/// the bad reply and a correction.
pub struct ForkRequest<'a> {
    pub messages: &'a [Msg],
    /// Rendered request body for the target's wire format (DeepSeek: the full Chat Completions body).
    pub body: &'a [u8],
    pub max_tokens: u32,
    /// 0 for the first call, 1 for the retry.
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryReply {
    pub text: String,
    /// The reply's reasoning, replayed on a retry (DeepSeek requires `reasoning_content` in tool mode).
    pub thinking: Option<String>,
    /// The reply contained a `tool_use` block (L12).
    pub tool_use: bool,
    pub model: String,
    pub usage: Option<UsageEv>,
}

/// The summarizer model call (purpose `compaction`). The provider layer implements it (WP10); tests
/// use a scripted stub.
pub trait Summarizer {
    fn summarize(&mut self, req: &ForkRequest) -> Result<SummaryReply, String>;
}

// ---------------------------------------------------------------------------------------------
// Summary parsing, validation and repair (§3.12.3 steps 4–5, §3.12.4)
// ---------------------------------------------------------------------------------------------

/// The model-written sections: heading path and id letter.
pub const SECTIONS: [(&str, char); 9] = [
    ("Intent", 'I'),
    ("Constraints and preferences", 'C'),
    ("Progress/Done", 'P'),
    ("Progress/In progress", 'P'),
    ("Progress/Blocked", 'P'),
    ("Decisions", 'D'),
    ("Open errors", 'E'),
    ("Next steps", 'N'),
    ("Critical details", 'K'),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item { id: String, text: String },
    Loose(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Summary {
    pub sections: [Vec<Entry>; 9],
    pub present: [bool; 9],
    pub retired: Vec<String>,
}

fn valid_item_id(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('I' | 'C' | 'P' | 'D' | 'E' | 'N' | 'K'))
        && s.len() > 1
        && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The `<summary …>…</summary>` block of a reply, if any.
pub fn summary_block(text: &str) -> Option<&str> {
    let s = text.find("<summary")?;
    let rest = &text[s..];
    let gt = rest.find('>')?;
    let end = rest.find("</summary>")?;
    if end < gt {
        return None;
    }
    Some(&rest[gt + 1..end])
}

pub fn parse_summary(inner: &str) -> Summary {
    let mut s = Summary::default();
    let mut cur: Option<usize> = None;
    let mut in_progress = false;
    for raw in inner.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if let Some(h) = t.strip_prefix("### ") {
            let h = h.trim();
            cur = match h {
                "Done" => Some(2),
                "In progress" => Some(3),
                "Blocked" => Some(4),
                _ if in_progress => None,
                _ => cur,
            };
            if let Some(i) = cur {
                s.present[i] = true;
            }
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            in_progress = false;
            cur = match h.trim() {
                "Intent" => Some(0),
                "Constraints and preferences" => Some(1),
                "Progress" => {
                    in_progress = true;
                    None
                }
                "Decisions" => Some(5),
                "Open errors" => Some(6),
                "Next steps" => Some(7),
                "Critical details" => Some(8),
                _ => None, // body-generated sections echoed by the model are ignored
            };
            if let Some(i) = cur {
                s.present[i] = true;
            }
            continue;
        }
        if let Some(r) = t.strip_prefix("Retired:") {
            s.retired = r
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(str::trim)
                .filter(|x| valid_item_id(x))
                .map(str::to_string)
                .collect();
            cur = None;
            continue;
        }
        let Some(sec) = cur else { continue };
        if t.is_empty() || t == "- none" {
            continue;
        }
        if let Some(rest) = t.strip_prefix("- [")
            && let Some(close) = rest.find(']')
        {
            let id = &rest[..close];
            if valid_item_id(id) {
                let text = rest[close + 1..].trim_start().to_string();
                s.sections[sec].push(Entry::Item {
                    id: id.to_string(),
                    text,
                });
                continue;
            }
        }
        match s.sections[sec].last_mut() {
            Some(Entry::Item { text, .. }) => {
                text.push('\n');
                text.push_str(line);
            }
            _ => s.sections[sec].push(Entry::Loose(line.to_string())),
        }
    }
    s
}

impl Summary {
    pub fn items(&self) -> impl Iterator<Item = (usize, &str, &str)> {
        self.sections.iter().enumerate().flat_map(|(i, es)| {
            es.iter().filter_map(move |e| match e {
                Entry::Item { id, text } => Some((i, id.as_str(), text.as_str())),
                Entry::Loose(_) => None,
            })
        })
    }

    pub fn ids(&self) -> Vec<String> {
        self.items().map(|(_, id, _)| id.to_string()).collect()
    }

    /// Model-written sections in canonical form.
    pub fn render_sections(&self) -> String {
        let mut out = String::new();
        for (i, (name, _)) in SECTIONS.iter().enumerate() {
            match name.split_once('/') {
                Some((parent, sub)) => {
                    if i == 2 {
                        out.push_str(&format!("## {parent}\n"));
                    }
                    out.push_str(&format!("### {sub}\n"));
                }
                None => out.push_str(&format!("## {name}\n")),
            }
            if self.sections[i].is_empty() {
                out.push_str("- none\n");
            }
            for e in &self.sections[i] {
                match e {
                    Entry::Item { id, text } => out.push_str(&format!("- [{id}] {text}\n")),
                    Entry::Loose(l) => {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}

/// What is wrong with a reply relative to the previous summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Problems {
    pub missing_ids: Vec<String>,
    pub duplicate_ids: Vec<String>,
    pub missing_sections: Vec<String>,
}

impl Problems {
    pub fn is_empty(&self) -> bool {
        self.missing_ids.is_empty()
            && self.duplicate_ids.is_empty()
            && self.missing_sections.is_empty()
    }

    /// The correction sent with the one retry.
    pub fn retry_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.missing_ids.is_empty() {
            parts.push(format!(
                "it dropped these ids without retiring them: {}",
                self.missing_ids.join(", ")
            ));
        }
        if !self.duplicate_ids.is_empty() {
            parts.push(format!(
                "these ids appear more than once: {}",
                self.duplicate_ids.join(", ")
            ));
        }
        if !self.missing_sections.is_empty() {
            parts.push(format!(
                "these sections are missing: {}",
                self.missing_sections.join(", ")
            ));
        }
        format!(
            "[init:compact] Your summary is not valid: {}. Send the whole summary again in the required format, keeping every id (or retiring it).",
            parts.join("; ")
        )
    }
}

/// `missing = ids(prev) − ids(r) − retired(r)`, duplicates and missing sections.
pub fn validate(prev: Option<&Summary>, r: &Summary) -> Problems {
    let mut p = Problems::default();
    let ids = r.ids();
    let mut seen = HashSet::new();
    for id in &ids {
        if !seen.insert(id.clone()) && !p.duplicate_ids.contains(id) {
            p.duplicate_ids.push(id.clone());
        }
    }
    if let Some(prev) = prev {
        let retired: HashSet<&String> = r.retired.iter().collect();
        for id in prev.ids() {
            if !seen.contains(&id) && !retired.contains(&id) && !p.missing_ids.contains(&id) {
                p.missing_ids.push(id);
            }
        }
    }
    for (i, (name, _)) in SECTIONS.iter().enumerate() {
        if !r.present[i] {
            p.missing_sections.push(name.to_string());
        }
    }
    p
}

/// Deterministic repair: re-insert each missing item verbatim from `prev` under its old heading with
/// ` (restored)`; renumber later duplicates to the next unused number of their letter; restore every
/// missing heading. Returns the repaired ids (restored ids, then new ids of renumbered items).
pub fn repair(prev: Option<&Summary>, r: &mut Summary) -> Vec<String> {
    let mut repaired = Vec::new();
    // Highest number per letter over everything known.
    let mut max: HashMap<char, u64> = HashMap::new();
    let note = |id: &str, max: &mut HashMap<char, u64>| {
        let c = id.chars().next().unwrap();
        let n: u64 = id[1..].parse().unwrap_or(0);
        let e = max.entry(c).or_insert(0);
        *e = (*e).max(n);
    };
    for id in r.ids().iter().chain(r.retired.iter()) {
        note(id, &mut max);
    }
    if let Some(p) = prev {
        for id in p.ids() {
            note(&id, &mut max);
        }
    }
    // Duplicates: keep the first occurrence.
    let mut seen = HashSet::new();
    for sec in r.sections.iter_mut() {
        for e in sec.iter_mut() {
            if let Entry::Item { id, .. } = e
                && !seen.insert(id.clone())
            {
                let c = id.chars().next().unwrap();
                let n = max.entry(c).or_insert(0);
                *n += 1;
                *id = format!("{c}{n}");
                seen.insert(id.clone());
                repaired.push(id.clone());
            }
        }
    }
    // Missing ids, restored verbatim under their previous heading.
    if let Some(p) = prev {
        let retired: HashSet<String> = r.retired.iter().cloned().collect();
        let mut restored = Vec::new();
        for (sec, id, text) in p.items() {
            if !seen.contains(id) && !retired.contains(id) {
                r.sections[sec].push(Entry::Item {
                    id: id.to_string(),
                    text: format!("{text} (restored)"),
                });
                seen.insert(id.to_string());
                restored.push(id.to_string());
            }
        }
        restored.extend(repaired);
        repaired = restored;
    }
    r.present = [true; 9];
    repaired
}

// ---------------------------------------------------------------------------------------------
// Owner messages and the artifact index (§3.12.4), generated by code
// ---------------------------------------------------------------------------------------------

/// One owner message or steer of a summarized span.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerMsg {
    pub id: String,
    pub ts: String,
    pub text: String,
}

pub fn owner_msg_of(l: &Loaded) -> Option<OwnerMsg> {
    let text = match &l.ev.body {
        EventBody::Message(m) if m.is_owner() => joined_text(&m.content, "\n"),
        EventBody::Steer(s) => joined_text(&s.content, "\n"),
        _ => return None,
    };
    Some(OwnerMsg {
        id: l.ev.id_hex(),
        ts: l.ev.ts.clone(),
        text,
    })
}

/// `## Owner messages (verbatim, newest last)`: newest 20, each up to 2,000 characters.
pub fn render_owner_msgs(msgs: &[OwnerMsg]) -> String {
    let mut out = String::from("## Owner messages (verbatim, newest last)\n");
    if msgs.is_empty() {
        out.push_str("- none\n");
        return out;
    }
    let skip = msgs.len().saturating_sub(OWNER_MSGS_KEPT);
    if skip > 0 {
        out.push_str(&format!("- (+{skip} older; see the log)\n"));
    }
    for m in &msgs[skip..] {
        let mut t: String = m.text.chars().take(OWNER_MSG_CHARS).collect();
        if m.text.chars().count() > OWNER_MSG_CHARS {
            t.push('…');
        }
        let ts = match m.ts.split_once('.') {
            Some((s, _)) => format!("{s}Z"),
            None => m.ts.clone(),
        };
        out.push_str(&format!("- {} {}: \"{}\"\n", m.id, ts, t));
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileRow {
    pub path: String,
    pub ops: String,
    pub n: u64,
    pub last: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventRow {
    pub text: String,
    pub event: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtRow {
    pub name: String,
    pub n: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Older {
    pub files: u64,
    pub spill: u64,
    pub commits: u64,
    pub exts: u64,
}

/// `compaction.artifacts`: structured, merged incrementally, never recomputed from model text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Artifacts {
    pub files: Vec<FileRow>,
    pub spill: Vec<EventRow>,
    pub commits: Vec<EventRow>,
    pub exts: Vec<ExtRow>,
    pub older: Older,
}

fn rel_path(p: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    let h = h.trim_end_matches('/');
    match p.strip_prefix(h).and_then(|r| r.strip_prefix('/')) {
        Some(r) if !h.is_empty() => r.to_string(),
        _ => p.to_string(),
    }
}

fn sha7(s: &str) -> &str {
    &s[..s.len().min(7)]
}

fn cap<T>(v: &mut Vec<T>, older: &mut u64) {
    if v.len() > ARTIFACT_ROWS {
        let n = v.len() - ARTIFACT_ROWS;
        v.drain(..n);
        *older += n as u64;
    }
}

impl Artifacts {
    pub fn from_value(v: &Value) -> Artifacts {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// new = merge(prev, span): rows touched by the span move to the end (newest last).
    pub fn merge(mut self, span: &[Loaded], home: &Path) -> Artifacts {
        for l in span {
            let id = l.ev.id_hex();
            match &l.ev.body {
                EventBody::Message(m) if m.is_assistant() => {
                    for (_, name, input) in m.tool_uses() {
                        let op = match name {
                            "read" => "r",
                            "write" => "w",
                            "edit" => "e",
                            "ext" => {
                                if let Some(n) = input.get("name").and_then(Value::as_str) {
                                    let mut row = match self.exts.iter().position(|r| r.name == n) {
                                        Some(i) => self.exts.remove(i),
                                        None => ExtRow {
                                            name: n.to_string(),
                                            n: 0,
                                        },
                                    };
                                    row.n += 1;
                                    self.exts.push(row);
                                }
                                continue;
                            }
                            _ => continue,
                        };
                        let Some(p) = input.get("path").and_then(Value::as_str) else {
                            continue;
                        };
                        let path = rel_path(p, home);
                        let mut row = match self.files.iter().position(|r| r.path == path) {
                            Some(i) => self.files.remove(i),
                            None => FileRow {
                                path,
                                ..Default::default()
                            },
                        };
                        let mut ops: BTreeSet<usize> = row
                            .ops
                            .split(',')
                            .filter_map(|o| ["r", "w", "e"].iter().position(|x| *x == o))
                            .collect();
                        ops.insert(["r", "w", "e"].iter().position(|x| *x == op).unwrap());
                        row.ops = ops
                            .iter()
                            .map(|i| ["r", "w", "e"][*i])
                            .collect::<Vec<_>>()
                            .join(",");
                        row.n += 1;
                        row.last = id.clone();
                        self.files.push(row);
                    }
                }
                EventBody::ToolResult(r) => {
                    if let Some(sp) = &r.spill_path {
                        self.spill.push(EventRow {
                            text: rel_path(sp, home),
                            event: id.clone(),
                        });
                    }
                }
                EventBody::SelfEdit(s) => {
                    let branch = s
                        .extra
                        .get("branch")
                        .and_then(Value::as_str)
                        .map(|b| format!(" {b}"))
                        .unwrap_or_default();
                    self.commits.push(EventRow {
                        text: format!("self_edit {} {}{branch}", s.tier, sha7(&s.sha)),
                        event: id,
                    });
                }
                EventBody::Gate(g) => {
                    let pass = match g.pass {
                        Some(true) => "pass",
                        Some(false) => "fail",
                        None => "pending",
                    };
                    self.commits.push(EventRow {
                        text: format!("gate {} {} {pass}", g.stage, sha7(&g.sha)),
                        event: id,
                    });
                }
                _ => {}
            }
        }
        cap(&mut self.files, &mut self.older.files);
        cap(&mut self.spill, &mut self.older.spill);
        cap(&mut self.commits, &mut self.older.commits);
        cap(&mut self.exts, &mut self.older.exts);
        self
    }

    pub fn render(&self) -> String {
        let mut out =
            String::from("## Artifacts\n### Files (newest last; ops r=read w=write e=edit)\n");
        if self.files.is_empty() {
            out.push_str("- none\n");
        } else {
            out.push_str("| path | ops | n | last event |\n|---|---|---|---|\n");
            for r in &self.files {
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    r.path, r.ops, r.n, r.last
                ));
            }
        }
        if self.older.files > 0 {
            out.push_str(&format!("(+{} older)\n", self.older.files));
        }
        let list = |out: &mut String, title: &str, rows: &[EventRow], older: u64| {
            out.push_str(title);
            if rows.is_empty() {
                out.push_str("- none\n");
            }
            for r in rows {
                out.push_str(&format!("- {} (event {})\n", r.text, r.event));
            }
            if older > 0 {
                out.push_str(&format!("(+{older} older)\n"));
            }
        };
        list(&mut out, "### Spill files\n", &self.spill, self.older.spill);
        list(
            &mut out,
            "### Commits and gates\n",
            &self.commits,
            self.older.commits,
        );
        out.push_str("### Extensions called\n");
        if self.exts.is_empty() {
            out.push_str("- none\n");
        }
        for r in &self.exts {
            out.push_str(&format!("- ext/{} ×{}\n", r.name, r.n));
        }
        if self.older.exts > 0 {
            out.push_str(&format!("(+{} older)\n", self.older.exts));
        }
        out
    }
}

/// Integer with `,` thousands separators.
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The `<summary>` block (stored as `compaction.summary`).
pub fn render_summary(
    n: u64,
    through: &str,
    model_part: &str,
    owner: &str,
    artifacts: &str,
    retired: &[String],
) -> String {
    let retired = if retired.is_empty() {
        String::new()
    } else {
        format!(" {}", retired.join(", "))
    };
    format!(
        "<summary v=\"1\" n=\"{n}\" through=\"{through}\">\n{model_part}{owner}{artifacts}Retired:{retired}\n</summary>"
    )
}

/// The stored summary text (P2): header lines, then the block.
pub fn render_summary_text(
    n: u64,
    through: &str,
    tokens_before: u64,
    sid: &str,
    summary: &str,
) -> String {
    format!(
        "[init:summary] Context compacted (compaction {n}; events up to {through} summarized; {} tokens before).\n\
         The full record is state/sessions/{sid}.jsonl; search it with grep or jq.\n\n{summary}",
        group_digits(tokens_before)
    )
}

/// Owner messages of a span (newest last).
pub fn owner_msgs_in(span: &[Loaded]) -> Vec<OwnerMsg> {
    span.iter()
        .filter(|l| matches!(l.kind(), Kind::Message | Kind::Steer))
        .filter_map(owner_msg_of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_relative_to_home() {
        let home = Path::new("/h/.init");
        assert_eq!(rel_path("/h/.init/body/src/x.rs", home), "body/src/x.rs");
        assert_eq!(rel_path("/h/.initx/y", home), "/h/.initx/y");
        assert_eq!(rel_path("work/z.rs", home), "work/z.rs");
    }

    #[test]
    fn digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(183_612), "183,612");
        assert_eq!(group_digits(1_000_000), "1,000,000");
    }

    #[test]
    fn prompt_focus() {
        let p = summary_prompt(None, false);
        assert!(p.contains("the harness adds those.\n\n<summary>"));
        let p = summary_prompt(Some("the plan"), true);
        assert!(p.contains(
            "adds those.\nThe owner asked this summary to focus on: the plan\nThe summary is over its budget: shorten item texts and retire finished items; keep ids.\n<summary>"
        ));
    }

    fn sample() -> &'static str {
        "x <summary>\n## Intent\n- [I1] Fix the test\n## Constraints and preferences\n- none\n## Progress\n### Done\n- [P1] ran tests\n  more detail\n### In progress\n- [P2] fixing\n### Blocked\n- none\n## Decisions\n- [D1] use std (because no deps)\n## Open errors\n- none\n## Next steps\n- [N1] rerun\n## Critical details\n- [K1] path src/a.rs\nRetired:\n</summary> y"
    }

    #[test]
    fn parse_and_render_roundtrip() {
        let s = parse_summary(summary_block(sample()).unwrap());
        assert_eq!(s.ids(), vec!["I1", "P1", "P2", "D1", "N1", "K1"]);
        assert!(s.present.iter().all(|p| *p));
        let r = s.render_sections();
        assert!(r.contains("### Done\n- [P1] ran tests\n  more detail\n### In progress"));
        let again = parse_summary(&r);
        assert_eq!(again, s);
    }

    /// T-CMP-2: dropped ids, duplicates and missing sections are found and repaired deterministically.
    #[test]
    fn validation_and_repair() {
        let prev = parse_summary(summary_block(sample()).unwrap());
        let bad = parse_summary(
            "## Intent\n- [I1] Fix\n- [I1] dup\n## Progress\n### Done\n- [P1] ran\nRetired: D1\n",
        );
        let p = validate(Some(&prev), &bad);
        assert_eq!(p.missing_ids, vec!["P2", "N1", "K1"]);
        assert_eq!(p.duplicate_ids, vec!["I1"]);
        assert!(p.missing_sections.contains(&"Decisions".to_string()));
        assert!(
            p.retry_text()
                .starts_with("[init:compact] Your summary is not valid: it dropped these ids")
        );
        let mut fixed = bad.clone();
        let repaired = repair(Some(&prev), &mut fixed);
        assert_eq!(repaired, vec!["P2", "N1", "K1", "I2"]);
        assert!(validate(Some(&prev), &fixed).is_empty());
        let text = fixed.render_sections();
        assert!(text.contains("- [P2] fixing (restored)"));
        assert!(text.contains("- [I2] dup"));
        assert!(text.contains("## Decisions\n- none\n"));
        // Deterministic.
        let mut again = bad.clone();
        assert_eq!(repair(Some(&prev), &mut again), repaired);
        assert_eq!(again.render_sections(), text);
    }
}
