//! `Predicts` clauses (evolution.md §3.10.1) and the capability claim (§3.9 step 11).
//!
//! ```abnf
//! predicts      = clause *( ";" *WSP clause )      ; 1 to 5 clauses
//! clause        = metric-clause / fixes-clause / risks-clause
//! metric-clause = metric [ "@" scope ] *WSP op *WSP target
//! fixes-clause  = "fixes" SP task-set
//! risks-clause  = "risks" SP task-set
//! task-set      = task-ref *( "," *WSP task-ref )  ; 1 to 10
//! task-ref      = "task:" task-id / "label:" label
//! metric        = "pass_rate" / "cost_usd" / "wall_s" / "cache_hit" / "turns"
//!               / "tokens_in" / "tokens_out" / "gap_ms_p50" / "body_tokens" / "ext_calls:" name
//! scope         = "all" / "pool" / "holdout" / "label:" label / "task:" task-id
//!               / "sealed"            ; M7: pass_rate on the Root's G4 slice only (evolution.md §3.10.1)
//! op            = "<=" / ">=" / "<" / ">"
//! target        = "parent" [ ( "+" / "-" ) number "%" ] / number
//! ```
//! Parsing is strict: no whitespace is allowed where the ABNF has none (e.g. before `;`).
//! Every value serializes as its trailer spelling.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use super::grammar::{self, string_serde};

/// Maximum clauses in one `Predicts` trailer.
pub const MAX_CLAUSES: usize = 5;
/// Maximum task refs in one `fixes` / `risks` set.
pub const MAX_TASK_REFS: usize = 10;

/// A metric (§3.10.1 table).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    PassRate,
    CostUsd,
    WallS,
    CacheHit,
    Turns,
    TokensIn,
    TokensOut,
    GapMsP50,
    BodyTokens,
    ExtCalls(String),
}

const SIMPLE_METRICS: [(&str, Metric); 9] = [
    ("pass_rate", Metric::PassRate),
    ("cost_usd", Metric::CostUsd),
    ("wall_s", Metric::WallS),
    ("cache_hit", Metric::CacheHit),
    ("turns", Metric::Turns),
    ("tokens_in", Metric::TokensIn),
    ("tokens_out", Metric::TokensOut),
    ("gap_ms_p50", Metric::GapMsP50),
    ("body_tokens", Metric::BodyTokens),
];

impl fmt::Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Metric::ExtCalls(name) = self {
            return write!(f, "ext_calls:{name}");
        }
        let name = SIMPLE_METRICS
            .iter()
            .find(|(_, m)| m == self)
            .map_or("?", |(n, _)| n);
        f.write_str(name)
    }
}

impl FromStr for Metric {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if let Some((_, m)) = SIMPLE_METRICS.iter().find(|(n, _)| *n == s) {
            return Ok(m.clone());
        }
        if let Some(name) = s.strip_prefix("ext_calls:") {
            if grammar::is_name(name) {
                return Ok(Metric::ExtCalls(name.to_string()));
            }
            return Err(format!(
                "ext_calls:<name> needs a name matching [a-z][a-z0-9-]{{0,31}}, got '{name}'"
            ));
        }
        Err(format!("unknown metric '{s}'"))
    }
}

/// A clause scope. An omitted scope is `all`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Scope {
    #[default]
    All,
    Pool,
    Holdout,
    Label(String),
    Task(String),
    /// M7 (evolution.md §3.10.1 scope `sealed`): a claim about S, scored from the slice's rounded
    /// `d_permille`. Valid only as `pass_rate@sealed (>=|>) parent[+N%]`.
    Sealed,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::All => f.write_str("all"),
            Scope::Pool => f.write_str("pool"),
            Scope::Holdout => f.write_str("holdout"),
            Scope::Label(l) => write!(f, "label:{l}"),
            Scope::Task(t) => write!(f, "task:{t}"),
            Scope::Sealed => f.write_str("sealed"),
        }
    }
}

impl FromStr for Scope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "all" => return Ok(Scope::All),
            "pool" => return Ok(Scope::Pool),
            "holdout" => return Ok(Scope::Holdout),
            "sealed" => return Ok(Scope::Sealed),
            _ => {}
        }
        if let Some(l) = s.strip_prefix("label:") {
            return if grammar::is_label(l) {
                Ok(Scope::Label(l.into()))
            } else {
                Err(format!("bad label '{l}'"))
            };
        }
        if let Some(t) = s.strip_prefix("task:") {
            return if grammar::is_task_id(t) {
                Ok(Scope::Task(t.into()))
            } else {
                Err(format!("bad task id '{t}'"))
            };
        }
        Err(format!("unknown scope '{s}'"))
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    Le,
    Ge,
    Lt,
    Gt,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Le => "<=",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Gt => ">",
        }
    }
    /// `<` or `>`.
    pub fn is_strict(self) -> bool {
        matches!(self, Op::Lt | Op::Gt)
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Op {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        [Op::Le, Op::Ge, Op::Lt, Op::Gt]
            .into_iter()
            .find(|o| o.as_str() == s)
            .ok_or_else(|| format!("unknown op '{s}'"))
    }
}

/// A clause target. Numbers are kept as their exact decimal text (`number = 1*DIGIT ["." 1*DIGIT]`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// `parent`
    Parent,
    /// `parent+N%`
    ParentPlus(String),
    /// `parent-N%`
    ParentMinus(String),
    /// a plain number
    Number(String),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Parent => f.write_str("parent"),
            Target::ParentPlus(n) => write!(f, "parent+{n}%"),
            Target::ParentMinus(n) => write!(f, "parent-{n}%"),
            Target::Number(n) => f.write_str(n),
        }
    }
}

impl FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if s == "parent" {
            return Ok(Target::Parent);
        }
        if let Some(rest) = s.strip_prefix("parent") {
            let pct = |r: &str| {
                r.strip_suffix('%')
                    .filter(|n| grammar::is_number(n))
                    .map(str::to_string)
            };
            if let Some(n) = rest.strip_prefix('+').and_then(pct) {
                return Ok(Target::ParentPlus(n));
            }
            if let Some(n) = rest.strip_prefix('-').and_then(pct) {
                return Ok(Target::ParentMinus(n));
            }
            return Err(format!(
                "target '{s}': expected parent, parent+N% or parent-N%"
            ));
        }
        if grammar::is_number(s) {
            return Ok(Target::Number(s.to_string()));
        }
        Err(format!(
            "target '{s}': expected parent[(+|-)N%] or a number"
        ))
    }
}

/// A `fixes` / `risks` task reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskRef {
    Task(String),
    Label(String),
}

impl fmt::Display for TaskRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskRef::Task(t) => write!(f, "task:{t}"),
            TaskRef::Label(l) => write!(f, "label:{l}"),
        }
    }
}

impl FromStr for TaskRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if let Some(t) = s.strip_prefix("task:").filter(|t| grammar::is_task_id(t)) {
            return Ok(TaskRef::Task(t.into()));
        }
        if let Some(l) = s.strip_prefix("label:").filter(|l| grammar::is_label(l)) {
            return Ok(TaskRef::Label(l.into()));
        }
        Err(format!(
            "task ref '{s}': expected task:<task-id> or label:<label>"
        ))
    }
}

string_serde!(Metric);
string_serde!(Scope);
string_serde!(Op);
string_serde!(Target);
string_serde!(TaskRef);

/// One `Predicts` clause.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Clause {
    Metric {
        metric: Metric,
        scope: Scope,
        op: Op,
        target: Target,
    },
    Fixes {
        tasks: Vec<TaskRef>,
    },
    Risks {
        tasks: Vec<TaskRef>,
    },
}

impl Clause {
    /// A baseline clause restates the accept rule (§3.10.1): `pass_rate >= parent`, `cost_usd <= parent`
    /// or `wall_s <= parent`, scope `all`, no percentage.
    pub fn is_baseline(&self) -> bool {
        match self {
            Clause::Metric {
                metric,
                scope: Scope::All,
                op,
                target: Target::Parent,
            } => matches!(
                (metric, op),
                (Metric::PassRate, Op::Ge) | (Metric::CostUsd, Op::Le) | (Metric::WallS, Op::Le)
            ),
            _ => false,
        }
    }

    /// The perf rule of the §3.8 matrix: a strict (`<` / `>`) clause in the improving direction on
    /// `cost_usd`, `wall_s`, `gap_ms_p50`, `turns`, `tokens_in`, `tokens_out` (`<`) or `cache_hit` (`>`).
    /// "Strict" is undefined in the doc; this is the reading recorded as a deviation.
    pub fn is_strict_perf(&self) -> bool {
        match self {
            Clause::Metric { metric, op, .. } => match metric {
                Metric::CostUsd
                | Metric::WallS
                | Metric::GapMsP50
                | Metric::Turns
                | Metric::TokensIn
                | Metric::TokensOut => *op == Op::Lt,
                Metric::CacheHit => *op == Op::Gt,
                _ => false,
            },
            _ => false,
        }
    }

    /// §3.9 step 11: the label of a `pass_rate@label:<l>` clause with an improving direction
    /// (`>` / `>=` with `parent+N%`, N > 0, or `parent` under `>`).
    pub fn capability_label(&self) -> Option<&str> {
        let Clause::Metric {
            metric: Metric::PassRate,
            scope: Scope::Label(label),
            op,
            target,
        } = self
        else {
            return None;
        };
        let improving = match (op, target) {
            (Op::Gt | Op::Ge, Target::ParentPlus(n)) => grammar::number_is_positive(n),
            (Op::Gt, Target::Parent) => true,
            _ => false,
        };
        improving.then_some(label.as_str())
    }

    /// M7: a pre-registered improvement on S, `pass_rate@sealed >= parent+N%` (or `>`) with N > 0,
    /// which the lineage ratchet requires (kernel/gate.md §3.9; evolution.md §3.10.1).
    pub fn is_sealed_gain(&self) -> bool {
        matches!(self, Clause::Metric { metric: Metric::PassRate, scope: Scope::Sealed, op: Op::Ge | Op::Gt, target: Target::ParentPlus(n) } if grammar::number_is_positive(n))
    }
}

/// M7: any clause pre-registers an improvement on S ([`Clause::is_sealed_gain`]).
pub fn predicts_sealed_gain(clauses: &[Clause]) -> bool {
    clauses.iter().any(Clause::is_sealed_gain)
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set = |tasks: &[TaskRef]| {
            tasks
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Clause::Metric {
                metric,
                scope: Scope::All,
                op,
                target,
            } => write!(f, "{metric} {op} {target}"),
            Clause::Metric {
                metric,
                scope,
                op,
                target,
            } => write!(f, "{metric}@{scope} {op} {target}"),
            Clause::Fixes { tasks } => write!(f, "fixes {}", set(tasks)),
            Clause::Risks { tasks } => write!(f, "risks {}", set(tasks)),
        }
    }
}

fn is_wsp(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn parse_task_set(s: &str) -> Result<Vec<TaskRef>, String> {
    let mut tasks = Vec::new();
    for (i, part) in s.split(',').enumerate() {
        let part = if i == 0 {
            part
        } else {
            part.trim_start_matches(is_wsp)
        };
        tasks.push(part.parse::<TaskRef>()?);
    }
    if tasks.len() > MAX_TASK_REFS {
        return Err(format!(
            "{} task refs; at most {MAX_TASK_REFS}",
            tasks.len()
        ));
    }
    Ok(tasks)
}

/// Parses one clause.
pub fn parse_clause(s: &str) -> Result<Clause, String> {
    if let Some(rest) = s.strip_prefix("fixes ") {
        return Ok(Clause::Fixes {
            tasks: parse_task_set(rest)?,
        });
    }
    if let Some(rest) = s.strip_prefix("risks ") {
        return Ok(Clause::Risks {
            tasks: parse_task_set(rest)?,
        });
    }
    // metric [ "@" scope ] *WSP op *WSP target
    let op_at = s
        .find(['<', '>'])
        .ok_or_else(|| "no operator (<=, >=, <, >)".to_string())?;
    let (lhs, rest) = s.split_at(op_at);
    let lhs = lhs.trim_end_matches(is_wsp);
    let (metric, scope) = match lhs.split_once('@') {
        Some((m, sc)) => (m, Some(sc)),
        None => (lhs, None),
    };
    let metric = metric.parse::<Metric>()?;
    let scope = match scope {
        Some(sc) => sc.parse::<Scope>()?,
        None => Scope::All,
    };
    let (op, rest) = if let Some(r) = rest.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = rest.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = rest.strip_prefix('<') {
        (Op::Lt, r)
    } else if let Some(r) = rest.strip_prefix('>') {
        (Op::Gt, r)
    } else {
        return Err("no operator".into());
    };
    let target = rest.trim_start_matches(is_wsp).parse::<Target>()?;
    // Scope `sealed` (evolution.md §3.10.1): metric pass_rate, op `>=` or `>`, target `parent` or
    // `parent+N%`; N is in percentage points of pass rate.
    if scope == Scope::Sealed
        && (metric != Metric::PassRate
            || !matches!(op, Op::Ge | Op::Gt)
            || !matches!(target, Target::Parent | Target::ParentPlus(_)))
    {
        return Err("scope sealed takes only pass_rate@sealed (>=|>) parent[+N%]".into());
    }
    Ok(Clause::Metric {
        metric,
        scope,
        op,
        target,
    })
}

/// The outcome of parsing one `Predicts` value: every clause that parsed, and one message per problem.
#[derive(Debug, Default)]
pub struct PredictsParse {
    pub clauses: Vec<Clause>,
    pub parse_errors: Vec<String>,
    /// Number of clauses written (parsed or not).
    pub count: usize,
}

/// Parses a `Predicts` value (§3.10.1). The caller maps `parse_errors` to `predicts.parse` and
/// `count > MAX_CLAUSES` to `predicts.too_many`.
pub fn parse_predicts(value: &str) -> PredictsParse {
    let mut out = PredictsParse::default();
    for (i, part) in value.split(';').enumerate() {
        out.count += 1;
        let part = if i == 0 {
            part
        } else {
            part.trim_start_matches(is_wsp)
        };
        match parse_clause(part) {
            Ok(c) => out.clauses.push(c),
            Err(e) => out
                .parse_errors
                .push(format!("clause {} '{part}': {e}", i + 1)),
        }
    }
    out
}

/// §3.9 step 11: sorted, de-duplicated capability labels of `clauses`.
pub fn capability_labels(clauses: &[Clause]) -> Vec<String> {
    clauses
        .iter()
        .filter_map(Clause::capability_label)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> Clause {
        parse_clause(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn spec_example_clauses() {
        let p = parse_predicts(
            "pass_rate@label:interactive > parent; gap_ms_p50@label:interactive <= parent-30%",
        );
        assert!(p.parse_errors.is_empty(), "{:?}", p.parse_errors);
        assert_eq!(p.clauses.len(), 2);
        assert_eq!(
            p.clauses[0],
            Clause::Metric {
                metric: Metric::PassRate,
                scope: Scope::Label("interactive".into()),
                op: Op::Gt,
                target: Target::Parent
            }
        );
        assert_eq!(
            p.clauses[1],
            Clause::Metric {
                metric: Metric::GapMsP50,
                scope: Scope::Label("interactive".into()),
                op: Op::Le,
                target: Target::ParentMinus("30".into())
            }
        );
        assert_eq!(
            capability_labels(&p.clauses),
            vec!["interactive".to_string()]
        );
    }

    #[test]
    fn every_form_parses_and_round_trips() {
        for s in [
            "pass_rate >= parent",
            "cost_usd<parent-10%",
            "wall_s@pool<=12.5",
            "cache_hit@holdout > parent+5%",
            "turns@task:2026-10-20-03 < 7",
            "tokens_in\t<=\tparent",
            "tokens_out <= parent",
            "body_tokens < parent-1%",
            "ext_calls:pty >= 1",
            "fixes task:2026-10-20-03, label:interactive,task:b",
            "risks label:git",
        ] {
            let c = one(s);
            let again = one(&c.to_string());
            assert_eq!(c, again, "{s}");
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Clause>(&json).unwrap(), c, "{json}");
        }
    }

    #[test]
    fn rejects() {
        for s in [
            "",
            "pass_rate",
            "pass_rate = parent",
            "pass_rate => parent",
            "pass_rate >= parent ",
            "pass_rate >= parent%",
            "pass_rate >= parent+%",
            "pass_rate >= parent*2",
            "pass_rate >= -1",
            "pass_rate@everything >= parent",
            "pass_rate@label:Bad >= parent",
            "speed >= parent",
            "ext_calls:Bad >= 1",
            "fixes",
            "fixes ",
            "fixes task:a,",
            "fixes task:a ,task:b",
            "fixes label:x, label:y, label:z, label:a, label:b, label:c, label:d, label:e, label:f, label:g, label:h",
            "risks foo",
        ] {
            assert!(parse_clause(s).is_err(), "{s:?} should not parse");
        }
        let p = parse_predicts("pass_rate >= parent ;cost_usd <= parent");
        assert_eq!(
            p.parse_errors.len(),
            1,
            "whitespace before ';' is not in the ABNF"
        );
        let p = parse_predicts("pass_rate > parent;");
        assert_eq!(
            (p.count, p.parse_errors.len()),
            (2, 1),
            "a trailing ';' is an empty clause"
        );
    }

    #[test]
    fn baseline_clauses() {
        for s in [
            "pass_rate >= parent",
            "cost_usd <= parent",
            "wall_s <= parent",
            "pass_rate@all >= parent",
        ] {
            assert!(one(s).is_baseline(), "{s}");
        }
        for s in [
            "pass_rate > parent",
            "pass_rate >= parent+1%",
            "pass_rate@pool >= parent",
            "cost_usd < parent",
            "cost_usd <= parent-1%",
            "turns <= parent",
            "fixes task:a",
        ] {
            assert!(!one(s).is_baseline(), "{s}");
        }
    }

    #[test]
    fn strict_perf_clauses() {
        for s in [
            "cost_usd < parent",
            "wall_s@pool < 30",
            "gap_ms_p50 < parent-10%",
            "cache_hit > parent",
            "turns < parent",
        ] {
            assert!(one(s).is_strict_perf(), "{s}");
        }
        for s in [
            "cost_usd <= parent-10%",
            "cost_usd > parent",
            "cache_hit < parent",
            "pass_rate > parent",
            "body_tokens < parent",
        ] {
            assert!(!one(s).is_strict_perf(), "{s}");
        }
    }

    #[test]
    fn capability_label_extraction() {
        let clauses: Vec<Clause> = [
            "pass_rate@label:b > parent",
            "pass_rate@label:a >= parent+5%",
            "pass_rate@label:a > parent+0.5%",
            "pass_rate@label:c >= parent",
            "pass_rate@label:d >= parent+0%",
            "pass_rate@label:e > parent-5%",
            "pass_rate@label:f > 0.9",
            "pass_rate@label:g < parent",
            "pass_rate@task:h > parent",
            "pass_rate > parent",
            "cost_usd@label:i < parent",
            "fixes label:j",
        ]
        .iter()
        .map(|s| one(s))
        .collect();
        assert_eq!(
            capability_labels(&clauses),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
