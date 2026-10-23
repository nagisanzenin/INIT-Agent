//! The validation algorithm (evolution.md §3.9): keys, values, the mandatory-trailer matrix, the
//! capability claim and the repo cross-checks. Every problem is collected; nothing short-circuits.

use std::collections::{BTreeMap, BTreeSet};

use super::grammar;
use super::predicts::{self, Clause, MAX_CLAUSES};
use super::repo::EvoRepo;
use super::subject::{Subject, parse_subject};
use super::trailers::{self, TrailerParse};
use super::{
    ChangeType, CompTag, ExtendMode, Measure, ReproRef, Trailers, UpstreamRef, Validated,
    ValidationError, codes,
};

/// The canonical trailer keys (§3.8), in matrix column order, then evolution-v2.md §4.2's `Proposal`,
/// then ADR-011's `Cap` and `Measure`.
pub const KEYS: [&str; 12] = [
    "Why",
    "Predicts",
    "Compensates-for",
    "Prefix-change",
    "Repro",
    "Retires",
    "Thins",
    "Upstream",
    "Reverts",
    "Proposal",
    "Cap",
    "Measure",
];

/// Maximum `Cap:` trailers in one commit (the Root refuses a change naming more than 96 caps).
pub const MAX_CAPS: usize = 64;

/// Maximum `Measure:` trailers in one commit.
pub const MAX_MEASURES: usize = 4;

/// Maximum `Repro` refs in one trailer.
pub const MAX_REPRO_REFS: usize = 5;

/// Maximum `Proposal` ids in one trailer (evolution-v2.md §4.2).
pub const MAX_PROPOSAL_IDS: usize = 4;

/// `Retires`, `Thins`, `Cap` and `Measure` may repeat; every other known key appears at most once.
pub fn is_repeatable(key: &str) -> bool {
    matches!(key, "Retires" | "Thins" | "Cap" | "Measure")
}

/// One cell of the mandatory-trailer matrix (§3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// M: must be present.
    Mandatory,
    /// C¹: mandatory only under the prefix rule (`Prefix-change: yes`).
    Conditional,
    /// o: may be present.
    Optional,
    /// –: must be absent.
    Forbidden,
}

/// The matrix: what type `t` requires of `key` on the gated path (v0.3, `extend.mode = "gated"`).
pub fn need(t: ChangeType, key: &str) -> Need {
    need_in(ExtendMode::Gated, t, key)
}

/// The matrix under `mode`. On the free path (ADR-011 "What is removed") only `Why` and
/// `Prefix-change` stay mandatory for every type: `Predicts`, `Compensates-for` and `Repro` become
/// optional metadata (a `–` cell stays forbidden), and the type-specific identity keys (`Retires`,
/// `Thins`, `Upstream`, `Reverts`) keep their cells, since they say what the commit is. `Cap` and
/// `Measure` are optional on both paths.
pub fn need_in(mode: ExtendMode, t: ChangeType, key: &str) -> Need {
    let cell = need_gated(t, key);
    match (mode, key, cell) {
        (ExtendMode::Free, "Predicts" | "Compensates-for" | "Repro", Need::Mandatory | Need::Conditional) => {
            Need::Optional
        }
        _ => cell,
    }
}

fn need_gated(t: ChangeType, key: &str) -> Need {
    use ChangeType::*;
    match key {
        "Cap" | "Measure" => Need::Optional,
        "Why" | "Predicts" | "Compensates-for" | "Prefix-change" => Need::Mandatory,
        "Repro" => match t {
            Fix | Guard => Need::Mandatory,
            Feat | Perf | Refactor | Skill => Need::Conditional,
            Retire | Thin | Upstream | Revert => Need::Forbidden,
        },
        "Retires" if t == Retire => Need::Mandatory,
        "Thins" if t == Thin => Need::Mandatory,
        "Upstream" if t == Upstream => Need::Mandatory,
        "Reverts" if t == Revert => Need::Mandatory,
        // evolution-v2.md §4.2: names the engine proposal(s) a commit implements.
        "Proposal" => match t {
            Upstream | Revert => Need::Forbidden,
            _ => Need::Optional,
        },
        _ => Need::Forbidden,
    }
}

/// The message-only part of [`Validated`]: §3.9 steps 4–9 and 11.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidatedMessage {
    pub change_type: ChangeType,
    pub scope: Option<String>,
    pub summary: String,
    pub trailers: Trailers,
    pub capability_labels: Vec<String>,
}

/// Everything learned from the message, valid or not, so the repo steps can still run.
#[derive(Debug, Default)]
pub(crate) struct MessageCheck {
    pub subject: Option<Subject>,
    pub trailers: Trailers,
    /// Canonical keys present in `L_git` (with any value).
    pub present: BTreeSet<&'static str>,
    /// Canonical keys whose value(s) parsed.
    pub valid: BTreeSet<&'static str>,
    pub capability_labels: Vec<String>,
    pub errors: Vec<ValidationError>,
}

fn err(errors: &mut Vec<ValidationError>, code: impl Into<String>, detail: impl Into<String>) {
    errors.push(ValidationError::new(code, detail));
}

fn parse_comp(v: &str) -> Result<CompTag, String> {
    const HINT: &str = "expected 'none', 'none (<note>)', '<slug>' or '<slug>: <note>' (slug: 3-40 of [a-z0-9-], note: 1-200 characters)";
    if v == "none" {
        return Ok(CompTag::default());
    }
    if let Some(rest) = v.strip_prefix("none (") {
        return match rest.strip_suffix(')') {
            Some(note) if grammar::is_text(note, 200) => Ok(CompTag {
                tag: "none".into(),
                note: Some(note.into()),
            }),
            _ => Err(HINT.into()),
        };
    }
    let (slug, note) = match v.split_once(':') {
        Some((slug, rest)) => match rest.strip_prefix(' ') {
            Some(note) if grammar::is_text(note, 200) => (slug, Some(note.to_string())),
            _ => return Err(HINT.into()),
        },
        None => (v, None),
    };
    if !grammar::is_slug(slug) {
        return Err(HINT.into());
    }
    Ok(CompTag {
        tag: slug.into(),
        note,
    })
}

fn parse_repro(v: &str) -> (Vec<ReproRef>, Vec<String>) {
    let parts: Vec<&str> = v.split(", ").collect();
    let mut refs = Vec::new();
    let mut problems = Vec::new();
    if parts.len() > MAX_REPRO_REFS {
        problems.push(format!("{} refs; at most {MAX_REPRO_REFS}", parts.len()));
    }
    for p in parts {
        match p.parse::<ReproRef>() {
            Ok(r) => refs.push(r),
            Err(e) => problems.push(e),
        }
    }
    (refs, problems)
}

/// `proposal = proposal-id *( "," SP proposal-id )`, 1 to [`MAX_PROPOSAL_IDS`] ids (evolution-v2.md §4.2).
fn parse_proposal(v: &str) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = v.split(", ").collect();
    if parts.len() > MAX_PROPOSAL_IDS {
        return Err(format!("{} ids; at most {MAX_PROPOSAL_IDS}", parts.len()));
    }
    match parts.iter().find(|p| !grammar::is_proposal_id(p)) {
        Some(p) => Err(format!(
            "'{p}': expected p-<6 digits>, ids separated by ', '"
        )),
        None => Ok(parts.into_iter().map(str::to_string).collect()),
    }
}

fn parse_upstream(v: &str) -> Result<UpstreamRef, String> {
    let parts: Vec<&str> = v.split(' ').collect();
    match parts.as_slice() {
        [tag, sha] | [tag, sha, "declined"]
            if grammar::is_seed_tag(tag) && grammar::is_hex(sha, 40) =>
        {
            Ok(UpstreamRef {
                tag: tag.to_string(),
                sha: sha.to_ascii_lowercase(),
                declined: parts.len() == 3,
            })
        }
        _ => Err("expected 'seed-v<semver> <40 hex> [declined]'".into()),
    }
}

/// §3.9 steps 4–9 and 11 on a raw message (everything after the first blank line of
/// `cat-file commit`).
///
/// Under `mode` (ADR-011: the free path relaxes the matrix and the rules that make
/// the agent supply evidence: the prefix rule, the non-trivial `Predicts` rule and the guard's named
/// weakness; every value that is present is still checked against its grammar).
pub(crate) fn check_message_in(parser: &dyn TrailerParse, msg: &str, mode: ExtendMode) -> MessageCheck {
    let mut c = MessageCheck::default();
    let norm = trailers::normalize(msg);
    let paras = trailers::paragraphs(&norm);

    // Step 4: subject.
    let first_line = norm.split('\n').next().unwrap_or("");
    if first_line.bytes().all(|b| b.is_ascii_whitespace()) {
        err(
            &mut c.errors,
            codes::SUBJECT,
            "the first line (the subject) is empty",
        );
    } else {
        let (subject, errors) = parse_subject(first_line);
        c.subject = subject;
        c.errors.extend(errors);
        if paras.first().is_some_and(|p| p.len() > 1) {
            err(
                &mut c.errors,
                codes::SUBJECT,
                "the subject must be a single line followed by a blank line",
            );
        }
    }

    // Step 5: git parse.
    let l_git = parser.parse(msg).unwrap_or_else(|e| {
        err(
            &mut c.errors,
            codes::IO,
            format!("git interpret-trailers: {e}"),
        );
        Vec::new()
    });

    // Step 6: strict block check.
    c.errors
        .extend(trailers::strict_block_check(&paras, l_git.len()));

    // Step 7: keys.
    let mut by_key: BTreeMap<&'static str, Vec<&str>> = BTreeMap::new();
    for (k, v) in &l_git {
        if let Some(canon) = KEYS.iter().find(|canon| **canon == k.as_str()) {
            by_key.entry(*canon).or_default().push(v.as_str());
        } else if let Some(canon) = KEYS.iter().find(|canon| canon.eq_ignore_ascii_case(k)) {
            err(
                &mut c.errors,
                codes::NONCANONICAL_KEY,
                format!("'{k}' must be spelled '{canon}'"),
            );
        } else {
            c.trailers
                .other
                .entry(k.clone())
                .or_default()
                .push(v.clone());
        }
    }
    for (&k, vals) in &by_key {
        c.present.insert(k);
        if !is_repeatable(k) && vals.len() > 1 {
            err(
                &mut c.errors,
                codes::DUPLICATE,
                format!("{k} appears {} times; it may appear once", vals.len()),
            );
        }
        if vals.iter().any(|v| v.trim().is_empty()) {
            err(
                &mut c.errors,
                codes::EMPTY,
                format!("{k} has an empty value"),
            );
        }
    }

    // Step 8: values (the first occurrence of a once-only key; every occurrence of a repeatable one).
    let first = |k: &str| {
        by_key
            .get(k)
            .and_then(|v| v.first())
            .copied()
            .filter(|v| !v.trim().is_empty())
    };
    let mut predicts_ok = false;
    if let Some(v) = first("Why") {
        if grammar::is_vchar_or_sp(v, 10, 500) {
            c.trailers.why = v.to_string();
            c.valid.insert("Why");
        } else {
            err(
                &mut c.errors,
                codes::invalid("Why"),
                format!(
                    "Why must be 10-500 printable ASCII characters; got {} bytes",
                    v.len()
                ),
            );
        }
    }
    if let Some(v) = first("Predicts") {
        let p = predicts::parse_predicts(v);
        for e in &p.parse_errors {
            err(&mut c.errors, codes::PREDICTS_PARSE, e.clone());
        }
        if p.count > MAX_CLAUSES {
            err(
                &mut c.errors,
                codes::PREDICTS_TOO_MANY,
                format!("{} clauses; at most {MAX_CLAUSES}", p.count),
            );
        }
        predicts_ok = p.parse_errors.is_empty() && p.count <= MAX_CLAUSES;
        if predicts_ok {
            c.valid.insert("Predicts");
        }
        c.trailers.predicts = p.clauses;
    }
    if let Some(v) = first("Compensates-for") {
        match parse_comp(v) {
            Ok(tag) => {
                c.trailers.compensates_for = tag;
                c.valid.insert("Compensates-for");
            }
            Err(e) => err(
                &mut c.errors,
                codes::invalid("Compensates-for"),
                format!("'{v}': {e}"),
            ),
        }
    }
    if let Some(v) = first("Prefix-change") {
        match v {
            "yes" | "no" => {
                c.trailers.prefix_change = v == "yes";
                c.valid.insert("Prefix-change");
            }
            _ => err(
                &mut c.errors,
                codes::invalid("Prefix-change"),
                format!("'{v}': expected 'yes' or 'no'"),
            ),
        }
    }
    if let Some(v) = first("Repro") {
        let (refs, problems) = parse_repro(v);
        for p in &problems {
            err(&mut c.errors, codes::invalid("Repro"), p.clone());
        }
        if problems.is_empty() {
            c.valid.insert("Repro");
        }
        c.trailers.repro = refs;
    }
    for key in ["Retires", "Thins"] {
        let mut ok = true;
        let mut ids = Vec::new();
        for v in by_key
            .get(key)
            .into_iter()
            .flatten()
            .filter(|v| !v.trim().is_empty())
        {
            if grammar::is_component_id(v) {
                ids.push(v.to_string());
            } else {
                ok = false;
                err(
                    &mut c.errors,
                    codes::invalid(key),
                    format!("'{v}' is not a component id (§3.8)"),
                );
            }
        }
        if ok && !ids.is_empty() {
            c.valid.insert(key);
        }
        if key == "Retires" {
            c.trailers.retires = ids;
        } else {
            c.trailers.thins = ids;
        }
    }
    if let Some(v) = first("Upstream") {
        match parse_upstream(v) {
            Ok(u) => {
                c.trailers.upstream = Some(u);
                c.valid.insert("Upstream");
            }
            Err(e) => err(
                &mut c.errors,
                codes::invalid("Upstream"),
                format!("'{v}': {e}"),
            ),
        }
    }
    if let Some(v) = first("Reverts") {
        if grammar::is_hex(v, 40) {
            c.trailers.reverts = Some(v.to_ascii_lowercase());
            c.valid.insert("Reverts");
        } else {
            err(
                &mut c.errors,
                codes::invalid("Reverts"),
                format!("'{v}': expected a 40-hex commit id"),
            );
        }
    }
    if let Some(v) = first("Proposal") {
        match parse_proposal(v) {
            Ok(ids) => {
                c.trailers.proposals = ids;
                c.valid.insert("Proposal");
            }
            Err(e) => err(&mut c.errors, codes::invalid("Proposal"), e),
        }
    }
    let all = |k: &str| {
        by_key
            .get(k)
            .into_iter()
            .flatten()
            .copied()
            .filter(|v| !v.trim().is_empty())
            .collect::<Vec<&str>>()
    };
    let caps = all("Cap");
    if !caps.is_empty() {
        let mut ok = true;
        if caps.len() > MAX_CAPS {
            ok = false;
            err(
                &mut c.errors,
                codes::invalid("Cap"),
                format!("{} Cap trailers; at most {MAX_CAPS}", caps.len()),
            );
        }
        for v in caps {
            if !grammar::is_cap_id(v) {
                ok = false;
                err(
                    &mut c.errors,
                    codes::invalid("Cap"),
                    format!(
                        "'{v}': expected <kind>/<name>, kind one of {}, name ^[a-z0-9][a-z0-9._/-]{{0,62}}$ (or prompt/GENESIS.md, prompt/SELF.md)",
                        grammar::CAP_KINDS.join(", ")
                    ),
                );
            } else if !c.trailers.caps.iter().any(|x| x == v) {
                c.trailers.caps.push(v.to_string());
            }
        }
        if ok {
            c.valid.insert("Cap");
        }
    }
    let measures = all("Measure");
    if !measures.is_empty() {
        let mut ok = true;
        if measures.len() > MAX_MEASURES {
            ok = false;
            err(
                &mut c.errors,
                codes::invalid("Measure"),
                format!("{} Measure trailers; at most {MAX_MEASURES}", measures.len()),
            );
        }
        for v in measures {
            match v.parse::<Measure>() {
                Ok(m) if !c.trailers.measure.contains(&m) => c.trailers.measure.push(m),
                Ok(_) => {}
                Err(e) => {
                    ok = false;
                    err(&mut c.errors, codes::invalid("Measure"), e);
                }
            }
        }
        if ok {
            c.valid.insert("Measure");
        }
    }

    // Step 9: the matrix and the prefix rule.
    if let Some(t) = c.subject.as_ref().map(|s| s.change_type) {
        let ty = t.as_str();
        let free = mode == ExtendMode::Free;
        for key in KEYS {
            match need_in(mode, t, key) {
                Need::Mandatory if !c.present.contains(key) => {
                    err(
                        &mut c.errors,
                        codes::missing(key),
                        format!("type {ty} requires {key}"),
                    );
                }
                Need::Forbidden if c.present.contains(key) => {
                    err(
                        &mut c.errors,
                        codes::forbidden(key),
                        format!("type {ty} must not carry {key}"),
                    );
                }
                _ => {}
            }
        }
        if !free
            && c.valid.contains("Prefix-change")
            && c.trailers.prefix_change
            && need(t, "Repro") == Need::Conditional
            && !c.present.contains("Repro")
        {
            err(
                &mut c.errors,
                codes::REPRO_REQUIRED_FOR_PREFIX,
                format!(
                    "type {ty} with Prefix-change: yes must cite the failure in Repro (no observed failure, no rule)"
                ),
            );
        }
        if !free
            && t == ChangeType::Guard
            && c.valid.contains("Compensates-for")
            && c.trailers.compensates_for.is_none()
        {
            err(
                &mut c.errors,
                codes::invalid("Compensates-for"),
                "type guard must name the weakness it compensates for: a slug, not none",
            );
        }
        if predicts_ok && !free {
            let clauses = &c.trailers.predicts;
            let trivial = clauses.iter().all(Clause::is_baseline);
            match t {
                ChangeType::Feat | ChangeType::Fix | ChangeType::Guard | ChangeType::Perf
                    if trivial =>
                {
                    err(
                        &mut c.errors,
                        codes::PREDICTS_TRIVIAL_ONLY,
                        format!("type {ty} needs at least one non-baseline Predicts clause"),
                    )
                }
                ChangeType::Perf if !clauses.iter().any(Clause::is_strict_perf) => err(
                    &mut c.errors,
                    codes::PREDICTS_TRIVIAL_ONLY,
                    "type perf needs at least one strict clause in the improving direction on cost_usd, wall_s, \
                     gap_ms_p50, turns, tokens_in, tokens_out ('<') or cache_hit ('>')",
                ),
                _ => {}
            }
        }
    }

    // Step 11: the capability claim.
    if predicts_ok {
        c.capability_labels = predicts::capability_labels(&c.trailers.predicts);
    }
    c
}

/// The pure part of the validator: §3.9 steps 4–9 and 11 on a raw commit message.
pub fn validate_message(
    parser: &dyn TrailerParse,
    msg: &str,
) -> Result<ValidatedMessage, Vec<ValidationError>> {
    validate_message_in(parser, msg, ExtendMode::Gated)
}

/// [`validate_message`] under `mode` (ADR-011).
pub fn validate_message_in(
    parser: &dyn TrailerParse,
    msg: &str,
    mode: ExtendMode,
) -> Result<ValidatedMessage, Vec<ValidationError>> {
    let mut c = check_message_in(parser, msg, mode);
    match c.subject.take() {
        Some(s) if c.errors.is_empty() => Ok(ValidatedMessage {
            change_type: s.change_type,
            scope: s.scope,
            summary: s.summary,
            trailers: c.trailers,
            capability_labels: c.capability_labels,
        }),
        _ => {
            if c.errors.is_empty() {
                err(&mut c.errors, codes::SUBJECT, "no subject");
            }
            Err(c.errors)
        }
    }
}

fn io_err(errors: &mut Vec<ValidationError>, what: &str, e: std::io::Error) {
    err(errors, codes::IO, format!("{what}: {e}"));
}

fn resolve(
    repo: &dyn EvoRepo,
    rev: &str,
    what: &str,
    errors: &mut Vec<ValidationError>,
) -> Option<String> {
    match repo.resolve_commit(rev) {
        Ok(Some(sha)) => Some(sha),
        Ok(None) => {
            err(
                errors,
                codes::IO,
                format!("{what} '{rev}' is not a commit in lineage"),
            );
            None
        }
        Err(e) => {
            io_err(errors, &format!("resolve {what}"), e);
            None
        }
    }
}

/// Step 2: the commit shape.
fn check_shape(
    repo: &dyn EvoRepo,
    t: Option<ChangeType>,
    upstream: Option<&UpstreamRef>,
    parent: &str,
    candidate: &str,
    errors: &mut Vec<ValidationError>,
) {
    let parents = match repo.parents(candidate) {
        Ok(p) => p,
        Err(e) => return io_err(errors, "rev-list --parents", e),
    };
    if t != Some(ChangeType::Upstream) {
        if parents.len() != 1 || parents[0] != parent {
            err(
                errors,
                codes::NOT_SINGLE_COMMIT,
                format!(
                    "the candidate must be one commit whose only parent is the running version {parent}; its parents are \
                     [{}]. Squash: git reset --soft origin/main && git commit",
                    parents.join(", ")
                ),
            );
        }
        return;
    }
    if parents.len() != 2 {
        err(
            errors,
            codes::UPSTREAM_SHAPE,
            format!(
                "an upstream commit is a merge with exactly two parents; it has {}",
                parents.len()
            ),
        );
    } else if parents[0] != parent {
        err(
            errors,
            codes::UPSTREAM_SHAPE,
            format!(
                "parent 1 is {}; it must be the running version {parent}",
                parents[0]
            ),
        );
    }
    let Some(up) = upstream else { return };
    match repo.upstream_tag_commit(&up.tag) {
        Ok(None) => err(
            errors,
            codes::UPSTREAM_UNKNOWN_TAG,
            format!("refs/upstream/{} does not exist in lineage", up.tag),
        ),
        Ok(Some(tag_sha)) => {
            if up.sha != tag_sha {
                err(
                    errors,
                    codes::UPSTREAM_SHAPE,
                    format!(
                        "Upstream names {} but refs/upstream/{} is {tag_sha}",
                        up.sha, up.tag
                    ),
                );
            }
            if parents.len() == 2 && parents[1] != tag_sha {
                err(
                    errors,
                    codes::UPSTREAM_SHAPE,
                    format!(
                        "parent 2 is {}; it must be refs/upstream/{} = {tag_sha}",
                        parents[1], up.tag
                    ),
                );
            }
        }
        Err(e) => io_err(errors, "rev-parse refs/upstream", e),
    }
}

fn is_t0_path(p: &str) -> bool {
    crate::scope::is_t0(p)
}

fn list_paths(paths: &[&String]) -> String {
    const SHOW: usize = 20;
    let mut s = paths
        .iter()
        .take(SHOW)
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOW {
        s.push_str(&format!(" and {} more", paths.len() - SHOW));
    }
    s
}

/// Deletion-shaped (§3.8 note 2): added ≤ max(10, deleted / 10) and no new files. `self.json` is
/// exempt from both (it is regenerated and checked at G1). Binary entries (`-`) count as 0 lines.
/// ADR-011 §6: `archive/**` and `quarantine/**` are exempt too (never loaded or built), so a `retire`
/// may move a cap there (`git mv skills/x archive/skill/x`) as well as delete it.
fn check_deletion_shaped(
    repo: &dyn EvoRepo,
    parent: &str,
    candidate: &str,
    errors: &mut Vec<ValidationError>,
) {
    match repo.numstat(parent, candidate) {
        Ok(entries) => {
            let (added, deleted) = entries
                .iter()
                .filter(|e| e.path != "self.json" && !crate::scope::is_inert(&e.path))
                .fold((0u64, 0u64), |(a, d), e| {
                    (a + e.added.unwrap_or(0), d + e.deleted.unwrap_or(0))
                });
            let allowed = std::cmp::max(10, deleted / 10);
            if added > allowed {
                err(
                    errors,
                    codes::RETIRE_NOT_DELETION,
                    format!(
                        "{added} lines added, {deleted} deleted; at most max(10, deleted/10) = {allowed} may be added"
                    ),
                );
            }
        }
        Err(e) => io_err(errors, "diff --numstat", e),
    }
    match repo.added_paths(parent, candidate) {
        Ok(paths) => {
            let new: Vec<&String> = paths
                .iter()
                .filter(|p| p.as_str() != "self.json" && !crate::scope::is_inert(p))
                .collect();
            if !new.is_empty() {
                err(
                    errors,
                    codes::RETIRE_NOT_DELETION,
                    format!("new files are not allowed: {}", list_paths(&new)),
                );
            }
        }
        Err(e) => io_err(errors, "diff --diff-filter=A", e),
    }
}

/// Step 10: cross-checks against the repo and state.
fn cross_checks(
    repo: &dyn EvoRepo,
    c: &MessageCheck,
    t: ChangeType,
    parent: &str,
    candidate: &str,
    changed: &[String],
    errors: &mut Vec<ValidationError>,
) {
    if t == ChangeType::Skill {
        let bad: Vec<&String> = changed
            .iter()
            .filter(|p| !is_t0_path(p) && !crate::scope::is_inert(p))
            .collect();
        if !bad.is_empty() {
            err(
                errors,
                codes::SKILL_PATHS,
                format!(
                    "a skill commit may change only skills/**, agents/**, SELF.md, GENESIS.md, archive/** and quarantine/**; it changes {}",
                    list_paths(&bad)
                ),
            );
        }
    }
    if matches!(t, ChangeType::Retire | ChangeType::Thin) {
        check_deletion_shaped(repo, parent, candidate, errors);
        let (key, ids) = if t == ChangeType::Retire {
            ("Retires", &c.trailers.retires)
        } else {
            ("Thins", &c.trailers.thins)
        };
        if c.valid.contains(key) {
            match repo.parent_components(parent) {
                Ok(components) => {
                    let components = components.unwrap_or_default();
                    for id in ids {
                        match components.get(id) {
                            None => err(
                                errors,
                                codes::RETIRE_UNKNOWN_COMPONENT,
                                format!("{key}: {id} is not a component in the parent's self.json"),
                            ),
                            Some(tag)
                                if t == ChangeType::Thin && c.valid.contains("Compensates-for") =>
                            {
                                if *tag != c.trailers.compensates_for.tag {
                                    err(
                                        errors,
                                        codes::THIN_TAG_MISMATCH,
                                        format!(
                                            "Compensates-for is '{}' but {id} is recorded with '{tag}'",
                                            c.trailers.compensates_for.tag
                                        ),
                                    );
                                }
                            }
                            Some(_) => {}
                        }
                    }
                }
                Err(e) => io_err(errors, "read the parent's self.json", e),
            }
        }
        if t == ChangeType::Thin {
            match repo.thinning_window_open() {
                Ok(true) => {}
                Ok(false) => err(
                    errors,
                    codes::THIN_NO_WINDOW,
                    "no thinning window is open (§3.12)",
                ),
                Err(e) => io_err(errors, "read the thinning state", e),
            }
        }
    }
    if t == ChangeType::Revert
        && let Some(target) = c.trailers.reverts.as_deref()
    {
        match repo.is_ancestor(target, parent) {
            Ok(false) => err(
                errors,
                codes::REVERT_MISMATCH,
                format!("Reverts {target} is not an ancestor of the running version {parent}"),
            ),
            Ok(true) => match repo.revert_tree_matches(parent, target, candidate) {
                Ok(true) => {}
                Ok(false) => err(
                    errors,
                    codes::REVERT_MISMATCH,
                    format!(
                        "the candidate's tree is not the tree of 'git revert {target}' on the running version"
                    ),
                ),
                Err(e) => io_err(errors, "revert tree check", e),
            },
            Err(e) => io_err(errors, "merge-base --is-ancestor", e),
        }
    }
    if need(t, "Repro") != Need::Forbidden && c.valid.contains("Repro") {
        for r in &c.trailers.repro {
            match repo.repro_exists(r) {
                Ok(true) => {}
                Ok(false) => err(
                    errors,
                    codes::REPRO_NOT_FOUND,
                    format!("Repro {r} does not exist"),
                ),
                Err(e) => io_err(errors, &format!("look up Repro {r}"), e),
            }
        }
    }
}

/// After step 10 (evolution-v2.md §4.2, §4.4): every `Proposal:` id names a kernel proposal record that
/// is `screened` and whose `chosen.sha` is this candidate (`proposal.not_found`, `proposal.state`).
fn check_proposals(
    repo: &dyn EvoRepo,
    c: &MessageCheck,
    t: ChangeType,
    candidate: &str,
    errors: &mut Vec<ValidationError>,
) {
    if need(t, "Proposal") == Need::Forbidden || !c.valid.contains("Proposal") {
        return;
    }
    for id in &c.trailers.proposals {
        let p = match repo.proposal(id) {
            Ok(Some(p)) => p,
            Ok(None) => {
                err(
                    errors,
                    codes::PROPOSAL_NOT_FOUND,
                    format!("Proposal {id} does not exist"),
                );
                continue;
            }
            Err(e) => {
                io_err(errors, &format!("read Proposal {id}"), e);
                continue;
            }
        };
        let mut problems = Vec::new();
        if p.state != "screened" {
            problems.push(format!("its state is '{}', not 'screened'", p.state));
        }
        match p.chosen_sha.as_deref() {
            None => problems.push("it has no chosen variant (chosen.sha)".to_string()),
            Some(sha) if !grammar::is_hex(sha, 40) => {
                problems.push(format!("its chosen.sha '{sha}' is not a 40-hex commit id"))
            }
            Some(sha) if !sha.eq_ignore_ascii_case(candidate) => problems.push(format!(
                "its chosen.sha is {sha}, not the candidate {candidate}"
            )),
            Some(_) => {}
        }
        if !problems.is_empty() {
            err(
                errors,
                codes::PROPOSAL_STATE,
                format!("Proposal {id}: {}", problems.join("; ")),
            );
        }
    }
}

/// The validator (§3.9): `parent` is the running version (`refs/heads/main`), `candidate` the fetched
/// candidate SHA. Returns every problem at once.
pub fn validate(
    repo: &dyn EvoRepo,
    parser: &dyn TrailerParse,
    parent: &str,
    candidate: &str,
) -> Result<Validated, Vec<ValidationError>> {
    validate_in(repo, parser, parent, candidate, ExtendMode::Gated)
}

/// [`validate`] under `mode` (ADR-011: the free path's relaxed matrix).
pub fn validate_in(
    repo: &dyn EvoRepo,
    parser: &dyn TrailerParse,
    parent: &str,
    candidate: &str,
    mode: ExtendMode,
) -> Result<Validated, Vec<ValidationError>> {
    let mut errors = Vec::new();
    // Step 1: resolve.
    let parent_sha = resolve(repo, parent, "parent", &mut errors);
    let cand_sha = resolve(repo, candidate, "candidate", &mut errors);
    let (Some(parent), Some(candidate)) = (parent_sha, cand_sha) else {
        return Err(errors);
    };

    // Steps 4-9 and 11 need the message; step 2 needs its type.
    let mut check = match repo.message(&candidate) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(msg) => check_message_in(parser, &msg, mode),
            Err(_) => {
                let mut c = MessageCheck::default();
                err(
                    &mut c.errors,
                    codes::SUBJECT,
                    "the commit message is not valid UTF-8",
                );
                c
            }
        },
        Err(e) => {
            let mut c = MessageCheck::default();
            io_err(&mut c.errors, "cat-file commit", e);
            c
        }
    };
    let t = check.subject.as_ref().map(|s| s.change_type);

    // Step 2: shape (step 3, identity, is supervise's).
    check_shape(
        repo,
        t,
        check.trailers.upstream.as_ref(),
        &parent,
        &candidate,
        &mut errors,
    );
    errors.append(&mut check.errors);

    // Step 10: cross-checks.
    let changed_paths = match repo.changed_paths(&parent, &candidate) {
        Ok(mut paths) => {
            paths.sort();
            paths.dedup();
            paths
        }
        Err(e) => {
            io_err(&mut errors, "diff --name-only", e);
            Vec::new()
        }
    };
    if let Some(t) = t {
        cross_checks(
            repo,
            &check,
            t,
            &parent,
            &candidate,
            &changed_paths,
            &mut errors,
        );
        // After step 10: the proposals the commit names.
        check_proposals(repo, &check, t, &candidate, &mut errors);
    }

    // Step 12.
    match check.subject {
        Some(s) if errors.is_empty() => Ok(Validated {
            sha: candidate,
            parent,
            change_type: s.change_type,
            scope: s.scope,
            summary: s.summary,
            trailers: check.trailers,
            changed_paths,
            capability_labels: check.capability_labels,
        }),
        _ => {
            if errors.is_empty() {
                err(&mut errors, codes::SUBJECT, "no subject");
            }
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_shape() {
        use ChangeType::*;
        // Every type requires the four universal keys.
        for t in ChangeType::ALL {
            for k in ["Why", "Predicts", "Compensates-for", "Prefix-change"] {
                assert_eq!(need(t, k), Need::Mandatory);
            }
        }
        assert_eq!(need(Fix, "Repro"), Need::Mandatory);
        assert_eq!(need(Feat, "Repro"), Need::Conditional);
        assert_eq!(need(Thin, "Repro"), Need::Forbidden);
        assert_eq!(need(Retire, "Retires"), Need::Mandatory);
        assert_eq!(need(Thin, "Retires"), Need::Forbidden);
        assert_eq!(need(Upstream, "Upstream"), Need::Mandatory);
        assert_eq!(need(Revert, "Reverts"), Need::Mandatory);
        assert_eq!(need(Feat, "Reverts"), Need::Forbidden);
        for t in ChangeType::ALL {
            let want = if matches!(t, Upstream | Revert) {
                Need::Forbidden
            } else {
                Need::Optional
            };
            assert_eq!(need(t, "Proposal"), want, "{t:?}");
        }
    }

    #[test]
    fn proposal_values() {
        assert_eq!(parse_proposal("p-000007").unwrap(), vec!["p-000007"]);
        assert_eq!(
            parse_proposal("p-000001, p-000002, p-000003, p-000004").unwrap(),
            vec!["p-000001", "p-000002", "p-000003", "p-000004"]
        );
        for bad in [
            "p-000001, p-000002, p-000003, p-000004, p-000005",
            "p-000001,p-000002",
            "p-000001,  p-000002",
            "p-7",
            "P-000007",
            "p-000007, ",
            "f-000003",
        ] {
            assert!(parse_proposal(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn compensates_values() {
        assert_eq!(parse_comp("none").unwrap(), CompTag::default());
        assert_eq!(
            parse_comp("none (new capability)").unwrap().note.as_deref(),
            Some("new capability")
        );
        let t = parse_comp("loop-detection: model loops on edits").unwrap();
        assert_eq!(
            (t.tag.as_str(), t.note.as_deref()),
            ("loop-detection", Some("model loops on edits"))
        );
        assert_eq!(parse_comp("loop-detection").unwrap().tag, "loop-detection");
        for bad in [
            "None", "none ()", "none(x)", "none x", "ab", "Loop", "loop:x", "loop: ",
        ] {
            assert!(parse_comp(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repro_and_upstream_values() {
        let (refs, p) = parse_repro(
            "task:2026-10-20-03, event:20261019-203015-9f3a2c#E4F1A2B3C4D5E6F7, trace:0123456789abcdef",
        );
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(
            refs[1].to_string(),
            "event:20261019-203015-9f3a2c#e4f1a2b3c4d5e6f7"
        );
        assert_eq!(refs[2], ReproRef::Trace("0123456789abcdef".into()));
        let (refs, p) = parse_repro("run:j-000031/p-repo-03-r1, friction:f-000003");
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(
            refs,
            vec![
                ReproRef::Run {
                    gate: "j-000031".into(),
                    run: "p-repo-03-r1".into()
                },
                ReproRef::Friction("f-000003".into())
            ]
        );
        for bad in [
            "run:j-31/p-repo-03-r1",
            "run:j-000031",
            "run:j-000031/",
            "run:j-000031/x-repo-03-r1",
            "run:j-000031/p-repo-03-r1/extra",
            "friction:f-3",
            "friction:000003",
            "lesson:l-000002",
        ] {
            assert_eq!(parse_repro(bad).1.len(), 1, "{bad}");
        }
        assert_eq!(parse_repro("task:a,task:b").1.len(), 1);
        assert_eq!(parse_repro("trace:0123456789abcde").1.len(), 1);
        assert_eq!(
            parse_repro("task:a, task:b, task:c, task:d, task:e, task:f")
                .1
                .len(),
            1
        );
        let sha = "a".repeat(40);
        let u = parse_upstream(&format!("seed-v0.3.0 {sha} declined")).unwrap();
        assert!(u.declined);
        assert!(
            !parse_upstream(&format!("seed-v0.3.0 {sha}"))
                .unwrap()
                .declined
        );
        for bad in [
            format!("seed-v0.3 {sha}"),
            format!("seed-v0.3.0 {sha} refused"),
            format!("seed-v0.3.0  {sha}"),
            "seed-v0.3.0".into(),
        ] {
            assert!(parse_upstream(&bad).is_err(), "{bad}");
        }
    }
}
