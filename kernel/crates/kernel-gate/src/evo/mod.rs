//! `evo`: the commit validator (evolution.md §3.8–§3.10.1).
//!
//! - [`subject`]: the subject line (§3.8 ABNF, §3.9 step 4);
//! - [`trailers`]: git as the trailer parser ([`TrailerParse`], [`GitTrailerParser`]) and the strict
//!   block check (§3.9 steps 5–6);
//! - [`predicts`]: the `Predicts` clause grammar (§3.10.1) and the capability claim (§3.9 step 11);
//! - [`validate`](mod@validate): keys, values and the mandatory-trailer matrix (§3.9 steps 7–9), the
//!   pure entry [`validate_message`] and the repo-backed entry [`validate()`] (steps 1–12);
//! - [`repo`]: the repo-dependent reads behind [`EvoRepo`] and the hardened-`$G` implementation
//!   [`GitCliRepo`] (supervise.md §3.11.2).
//!
//! Every problem is collected, never short-circuited (§3.9), and reported as a [`ValidationError`]
//! with a dotted code from §4.2.

use std::collections::BTreeMap;

pub mod grammar;
pub mod predicts;
mod proc;
pub mod repo;
pub mod subject;
pub mod trailers;
pub mod validate;

#[cfg(test)]
mod tests;

pub use predicts::{Clause, Metric, Op, Scope, Target, TaskRef, predicts_sealed_gain};
pub use repo::{EvoRepo, GitCliRepo, NumstatEntry, ProposalView};
pub use trailers::{GitTrailerParser, TrailerParse};
pub use validate::{
    ValidatedMessage, validate, validate_in, validate_message, validate_message_in,
};

/// Commit change type (evolution.md §3.8). Shared with [`crate::accept`] (the `perf` rule).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Feat,
    Fix,
    Perf,
    Guard,
    Refactor,
    Skill,
    Retire,
    Thin,
    Upstream,
    Revert,
}

impl ChangeType {
    /// Every type, in §3.8's table order.
    pub const ALL: [ChangeType; 10] = [
        ChangeType::Feat,
        ChangeType::Fix,
        ChangeType::Perf,
        ChangeType::Guard,
        ChangeType::Refactor,
        ChangeType::Skill,
        ChangeType::Retire,
        ChangeType::Thin,
        ChangeType::Upstream,
        ChangeType::Revert,
    ];

    /// The subject spelling (`feat`, `fix`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeType::Feat => "feat",
            ChangeType::Fix => "fix",
            ChangeType::Perf => "perf",
            ChangeType::Guard => "guard",
            ChangeType::Refactor => "refactor",
            ChangeType::Skill => "skill",
            ChangeType::Retire => "retire",
            ChangeType::Thin => "thin",
            ChangeType::Upstream => "upstream",
            ChangeType::Revert => "revert",
        }
    }

    /// Parses the exact (case-sensitive) subject spelling.
    pub fn parse(s: &str) -> Option<ChangeType> {
        ChangeType::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// The validator's result (evolution.md §3.9 step 12). Stored verbatim with sorted keys in the gate
/// note and `vault/lineage.jsonl`; use [`Validated::to_sorted_json`] for that form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Validated {
    pub sha: String,
    pub parent: String,
    pub change_type: ChangeType,
    pub scope: Option<String>,
    pub summary: String,
    pub trailers: Trailers,
    /// Sorted; input to gate.md's scope decision.
    pub changed_paths: Vec<String>,
    /// Sorted and de-duplicated; gate.md §3.9.1 `capability_gain` (§3.9 step 11).
    pub capability_labels: Vec<String>,
}

impl Validated {
    /// Compact JSON with every object's keys sorted (serde_json without `preserve_order`).
    pub fn to_sorted_json(&self) -> String {
        to_sorted_json(self)
    }
}

/// Serializes through `serde_json::Value`, whose maps are `BTreeMap`s, so keys come out sorted.
pub fn to_sorted_json<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(value) => value.to_string(),
        Err(e) => format!("{{\"error\":{:?}}}", e.to_string()),
    }
}

/// Parsed trailers (evolution.md §3.8). Keys the kernel does not know are kept in `other`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Trailers {
    pub why: String,
    pub predicts: Vec<Clause>,
    pub compensates_for: CompTag,
    pub prefix_change: bool,
    pub repro: Vec<ReproRef>,
    pub retires: Vec<String>,
    pub thins: Vec<String>,
    pub upstream: Option<UpstreamRef>,
    pub reverts: Option<String>,
    /// `Proposal:` ids (`p-` + 6 digits, evolution-v2.md §4.2), in message order. Omitted from the JSON
    /// when empty, so records of commits without the trailer keep their exact v0.3 form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<String>,
    /// `Cap:` ids (ADR-011 §5.1; repeatable), in message order, each once. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
    /// `Measure:` requests (ADR-011 §8; repeatable), in message order, each once. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measure: Vec<Measure>,
    /// Unknown keys (`Signed-off-by`, …): sorted by key, values in message order. No meaning to the kernel.
    pub other: BTreeMap<String, Vec<String>>,
}

/// Charter `extend.mode` (ADR-011 decision 11). `Gated` keeps the v0.3 path and the full
/// mandatory-trailer matrix; `Free` relaxes the matrix to `Why` and `Prefix-change` (ADR-011 "What
/// is removed": `Predicts`, `Repro` and `Compensates-for` become optional metadata).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ExtendMode {
    #[default]
    Gated,
    Free,
}

impl ExtendMode {
    /// `"free"` → `Free`; anything else (absent, `"gated"`, unknown) → `Gated`, the safe default.
    pub fn parse(s: Option<&str>) -> ExtendMode {
        if s == Some("free") {
            ExtendMode::Free
        } else {
            ExtendMode::Gated
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExtendMode::Gated => "gated",
            ExtendMode::Free => "free",
        }
    }
}

/// One `Measure:` request (ADR-011 §8): an instrument the agent asks for after admission. None of
/// them decides admission. Serialized in its trailer spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Measure {
    /// `g4`: G4 v2 after admission, report only.
    G4,
    /// `g4 gate`: the agent's self-imposed gate (the kernel holds its own `lkg` step until the result).
    G4Gate,
    /// `exp:<family>`: the agent's admitted task family, candidate against parent, advisory.
    Exp(String),
    /// `gog`: the Root's gate-of-gates on a kernel candidate.
    Gog,
    /// `card:<id>` / `card:<id> gate` (ADR-013): a Root observation-card reading of this change (owner turns before
    /// and after it); `gate`: the agent's self-imposed gate, rolled back by the Root unless the reading is better.
    Card(String, bool),
}

impl std::fmt::Display for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Measure::G4 => f.write_str("g4"),
            Measure::G4Gate => f.write_str("g4 gate"),
            Measure::Exp(fam) => write!(f, "exp:{fam}"),
            Measure::Gog => f.write_str("gog"),
            Measure::Card(id, false) => write!(f, "card:{id}"),
            Measure::Card(id, true) => write!(f, "card:{id} gate"),
        }
    }
}

impl std::str::FromStr for Measure {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "g4" => Ok(Measure::G4),
            "g4 gate" => Ok(Measure::G4Gate),
            "gog" => Ok(Measure::Gog),
            _ if s.starts_with("card:") => {
                let (id, gate) = s["card:".len()..].strip_suffix(" gate").map_or((&s["card:".len()..], false), |i| (i, true));
                if (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
                    Ok(Measure::Card(id.to_string(), gate))
                } else {
                    Err(format!("'{s}': the card id must match [a-z_]{{1,32}}"))
                }
            }
            _ => match s.strip_prefix("exp:") {
                Some(fam) if grammar::is_family_id(fam) => Ok(Measure::Exp(fam.to_string())),
                Some(_) => Err(format!(
                    "'{s}': the family id must match [A-Za-z0-9][A-Za-z0-9._-]{{0,63}}"
                )),
                None => Err(format!(
                    "'{s}': expected 'g4', 'g4 gate', 'gog', 'card:<id>[ gate]' or 'exp:<family>'"
                )),
            },
        }
    }
}

grammar::string_serde!(Measure);

/// `Compensates-for` (§3.8): `none [(<note>)]` or `<slug>[: <note>]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompTag {
    /// `"none"` or the lowercase weakness slug.
    pub tag: String,
    /// The free-text note, if any.
    pub note: Option<String>,
}

impl Default for CompTag {
    fn default() -> Self {
        CompTag {
            tag: "none".into(),
            note: None,
        }
    }
}

impl CompTag {
    /// True for `none` (no weakness named).
    pub fn is_none(&self) -> bool {
        self.tag == "none"
    }
}

/// One `Repro` reference (§3.8, evolution-v2.md §4.2). Serialized in its trailer spelling (`task:<id>`,
/// `event:<session>#<16 hex>`, `trace:<16 hex>`, `run:<gate-id>/<run-id>`, `friction:f-<6 digits>`);
/// hex is stored lowercase. `Run` is a pool run (`gate` = `j-` + 6 digits, `run` = gate.md §3.1's
/// `<arm>-<task_id>-r<k>`); `Friction` holds the full cluster id (`f-000003`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReproRef {
    Task(String),
    Event { session: String, id: String },
    Trace(String),
    Run { gate: String, run: String },
    Friction(String),
}

impl std::fmt::Display for ReproRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReproRef::Task(id) => write!(f, "task:{id}"),
            ReproRef::Event { session, id } => write!(f, "event:{session}#{id}"),
            ReproRef::Trace(id) => write!(f, "trace:{id}"),
            ReproRef::Run { gate, run } => write!(f, "run:{gate}/{run}"),
            ReproRef::Friction(id) => write!(f, "friction:{id}"),
        }
    }
}

impl std::str::FromStr for ReproRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if let Some(id) = s.strip_prefix("task:") {
            if grammar::is_task_id(id) {
                return Ok(ReproRef::Task(id.to_string()));
            }
            return Err(format!(
                "'{s}': task id must match [A-Za-z0-9][A-Za-z0-9._-]{{0,63}}"
            ));
        }
        if let Some(rest) = s.strip_prefix("event:") {
            if let Some((session, id)) = rest.split_once('#')
                && grammar::is_session_id(session)
                && grammar::is_hex(id, 16)
            {
                return Ok(ReproRef::Event {
                    session: session.to_string(),
                    id: id.to_ascii_lowercase(),
                });
            }
            return Err(format!("'{s}': expected event:<session-id>#<16 hex>"));
        }
        if let Some(id) = s.strip_prefix("trace:") {
            if grammar::is_hex(id, 16) {
                return Ok(ReproRef::Trace(id.to_ascii_lowercase()));
            }
            return Err(format!("'{s}': expected trace:<16 hex>"));
        }
        if let Some(rest) = s.strip_prefix("run:") {
            if let Some((gate, run)) = rest.split_once('/')
                && grammar::is_gate_id(gate)
                && grammar::is_run_id(run)
            {
                return Ok(ReproRef::Run {
                    gate: gate.to_string(),
                    run: run.to_string(),
                });
            }
            return Err(format!(
                "'{s}': expected run:j-<6 digits>/<arm>-<task id>-r<k> (arm c or p)"
            ));
        }
        if let Some(id) = s.strip_prefix("friction:") {
            if grammar::is_friction_id(id) {
                return Ok(ReproRef::Friction(id.to_string()));
            }
            return Err(format!("'{s}': expected friction:f-<6 digits>"));
        }
        Err(format!(
            "'{s}': expected task:, event:, trace:, run: or friction:"
        ))
    }
}

grammar::string_serde!(ReproRef);

/// `Upstream: <tag> <40 hex> [declined]` (§3.8). `sha` is stored lowercase.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpstreamRef {
    pub tag: String,
    pub sha: String,
    pub declined: bool,
}

/// One validation problem (§4.2). `code` is a `String` because of `trailer.missing:<Key>` and friends.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub detail: String,
}

impl ValidationError {
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        ValidationError {
            code: code.into(),
            detail: detail.into(),
        }
    }

    /// True for a kernel-side failure ([`codes::IO`]) rather than a problem with the commit.
    pub fn is_internal(&self) -> bool {
        self.code == codes::IO
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

/// The dotted codes of evolution.md §4.2, plus this module's two additions (see each doc comment).
pub mod codes {
    pub const NOT_SINGLE_COMMIT: &str = "promote.not_single_commit";
    pub const SUBJECT: &str = "trailer.subject";
    pub const SUBJECT_TOO_LONG: &str = "trailer.subject_too_long";
    pub const BLOCK_MISSING: &str = "trailer.block_missing";
    pub const BLOCK_MIXED: &str = "trailer.block_mixed";
    pub const PARSER_MISMATCH: &str = "trailer.parser_mismatch";
    pub const NONCANONICAL_KEY: &str = "trailer.noncanonical_key";
    pub const DUPLICATE: &str = "trailer.duplicate";
    pub const EMPTY: &str = "trailer.empty";
    pub const REPRO_REQUIRED_FOR_PREFIX: &str = "trailer.repro_required_for_prefix";
    pub const PREDICTS_PARSE: &str = "predicts.parse";
    pub const PREDICTS_TOO_MANY: &str = "predicts.too_many";
    pub const PREDICTS_TRIVIAL_ONLY: &str = "predicts.trivial_only";
    pub const REPRO_NOT_FOUND: &str = "repro.not_found";
    pub const SKILL_PATHS: &str = "skill.paths";
    pub const RETIRE_NOT_DELETION: &str = "retire.not_deletion";
    pub const RETIRE_UNKNOWN_COMPONENT: &str = "retire.unknown_component";
    pub const THIN_TAG_MISMATCH: &str = "thin.tag_mismatch";
    pub const THIN_NO_WINDOW: &str = "thin.no_window";
    pub const UPSTREAM_SHAPE: &str = "upstream.shape";
    pub const UPSTREAM_UNKNOWN_TAG: &str = "upstream.unknown_tag";
    pub const REVERT_MISMATCH: &str = "revert.mismatch";
    /// evolution-v2.md §4.4: a `Proposal:` id has no `state/kernel/evo/proposals/<id>.json`.
    pub const PROPOSAL_NOT_FOUND: &str = "proposal.not_found";
    /// evolution-v2.md §4.4: the proposal is not `screened`, or its `chosen.sha` is not the candidate.
    pub const PROPOSAL_STATE: &str = "proposal.state";
    /// Not in §4.2 (deviation): a known key whose value breaks its §3.8 ABNF (`Why`, `Repro`, …).
    /// §3.9 step 8 names no code for it; this follows the `trailer.missing:<Key>` pattern.
    pub const INVALID_PREFIX: &str = "trailer.invalid:";
    /// Not in §4.2 (deviation): the kernel could not run git or read its own state. Not the agent's fault;
    /// supervise should map it to an internal error rather than a validation refusal.
    pub const IO: &str = "evo.io";

    /// `trailer.missing:<Key>`.
    pub fn missing(key: &str) -> String {
        format!("trailer.missing:{key}")
    }
    /// `trailer.forbidden:<Key>`.
    pub fn forbidden(key: &str) -> String {
        format!("trailer.forbidden:{key}")
    }
    /// `trailer.invalid:<Key>` (see [`INVALID_PREFIX`]).
    pub fn invalid(key: &str) -> String {
        format!("{INVALID_PREFIX}{key}")
    }
}
