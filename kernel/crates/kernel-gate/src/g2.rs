//! G2, the boot probe (gate.md §3.5): the pass criteria, evaluated on what the two probes and the
//! proxy observed. The probes themselves are supervise's `probe()`; the driver lives in
//! `kernel-supervise::promote`. Everything here is pure, so each criterion is unit-tested.

use serde::Serialize;
use serde_json::{Value, json};

/// `ready` deadline of a probe (§3.5 criterion 1).
pub const READY_TIMEOUT_S: u64 = 90;
/// The probe window after `ready`.
pub const WINDOW_S: u64 = 15;
/// The parent probe is retried once after this many seconds (§3.5 "If arm p fails").
pub const PARENT_RETRY_S: u64 = 30;

/// How a probe failed before or at `ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartFail {
    /// No `ready` within 90 s.
    Timeout,
    /// The body exited before `ready` (code, signal).
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// The probe could not be started (root, spawn, too many probes).
    Infra(String),
}

/// What one arm's probe produced (`ProbeOutcome` plus the proxy's `ScopeSummary`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArmObs {
    pub sha: String,
    pub start_fail: Option<StartFail>,
    pub ready_ms: Option<u64>,
    /// The 15 s probe window passed: no exit, at least two heartbeats.
    pub window_ok: bool,
    /// The body exited during the window (a crash after `ready`).
    pub window_exit: bool,
    /// Exit code after supervise's `stop` (`None` if it had to be killed).
    pub stop_code: Option<i32>,
    /// `ScopeSummary` (kernel/proxy.md §4.4) as JSON; `Null` when the proxy saw nothing.
    pub scope: Value,
    /// The last `boot` event of the probe's session log.
    pub last_boot_event: Option<Value>,
}

impl ArmObs {
    fn first(&self, key: &str) -> Option<&str> {
        self.scope.get("first")?.get(key)?.as_str()
    }
    fn u(&self, key: &str) -> Option<u64> {
        self.scope.get(key).and_then(Value::as_u64)
    }
}

/// Continuity of the candidate's first request with the running body's last one (log-context X5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuity {
    Pass,
    /// The live scope had no request for the copied session.
    NotApplicable,
    /// No candidate request with the session id and enough elements (a warning).
    NotObserved,
    /// Element `index` differs at byte `offset` of its ICJ form.
    Fail {
        index: usize,
        offset: usize,
    },
    /// Waived by `Prefix-change: yes`.
    Waived,
}

impl Continuity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Continuity::Pass => "pass",
            Continuity::NotApplicable => "n/a",
            Continuity::NotObserved => "not_observed",
            Continuity::Fail { .. } => "fail",
            Continuity::Waived => "waived",
        }
    }
}

/// Re-serialize one message-list element as canonical JSON (sorted keys, no whitespace). `None` if it
/// does not parse.
pub fn canonical(elem: &[u8]) -> Option<Vec<u8>> {
    serde_json::from_slice::<Value>(elem)
        .ok()
        .map(|v| v.to_string().into_bytes())
}

/// The X5 check (§3.5 criterion 7). `r_p` is the running body's last request's message list (`None`:
/// the live scope has none for the session); `r_c` are the candidate's requests for that session, in
/// order. The first `r_c` with at least `len(r_p)` elements is compared, element by element.
pub fn continuity(r_p: Option<&[Vec<u8>]>, r_c: &[Vec<Vec<u8>>]) -> Continuity {
    let Some(r_p) = r_p else {
        return Continuity::NotApplicable;
    };
    let Some(c) = r_c.iter().find(|c| c.len() >= r_p.len()) else {
        return Continuity::NotObserved;
    };
    for (i, (a, b)) in r_p.iter().zip(c.iter()).enumerate() {
        let (a, b) = (
            canonical(a).unwrap_or_else(|| a.clone()),
            canonical(b).unwrap_or_else(|| b.clone()),
        );
        if a != b {
            return Continuity::Fail {
                index: i,
                offset: first_diff(&a, &b),
            };
        }
    }
    Continuity::Pass
}

/// The first differing byte offset (the shorter length when one is a prefix of the other).
pub fn first_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()))
}

/// Everything the criteria read.
#[derive(Debug, Clone, Default)]
pub struct G2Input {
    pub c: ArmObs,
    pub p: ArmObs,
    /// G1's `tools.sha256` of the candidate's regenerated `self.json`.
    pub self_tools_sha256: Option<String>,
    /// G1's per-tool `(name, schema_sha256)`.
    pub self_tools: Vec<(String, String)>,
    /// `Prefix-change: yes` in the validated trailers.
    pub prefix_change: bool,
    pub continuity: Option<Continuity>,
    /// ICJ head objects of both first requests (from the trace store), for the `prefix_hash` offset.
    pub heads: Option<(Vec<u8>, Vec<u8>)>,
}

/// One failed criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// `gate.g2.<reason>`.
    pub code: String,
    pub detail: String,
}

/// G2's result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct G2Outcome {
    pub pass: bool,
    /// Arm p failed criteria 1–3: `infra:parent_probe`, not the candidate's fault.
    pub parent_failed: bool,
    /// Every failed criterion of arm c, in criterion order (the first is reported).
    pub failures: Vec<Failure>,
    pub warnings: Vec<String>,
    /// The `stages.G2` object of the gate record (§3.13).
    pub record: Value,
}

impl G2Outcome {
    /// The reported failure: the parent's infra failure first, else arm c's first criterion.
    pub fn code(&self) -> Option<&str> {
        if self.parent_failed {
            return Some("gate.g2.parent_probe");
        }
        self.failures.first().map(|f| f.code.as_str())
    }
    pub fn detail(&self) -> String {
        if self.parent_failed {
            return "the running version's probe failed (infra, not the candidate's fault)".into();
        }
        self.failures
            .first()
            .map(|f| f.detail.clone())
            .unwrap_or_default()
    }
}

/// Criteria 1–3 for one arm: `None` if they pass.
fn boot_criteria(a: &ArmObs) -> Vec<Failure> {
    let mut f = Vec::new();
    let fail = |code: &str, d: String| Failure {
        code: format!("gate.g2.{code}"),
        detail: d,
    };
    match &a.start_fail {
        Some(StartFail::Timeout) => f.push(fail(
            "probe_timeout",
            format!("no `ready` within {READY_TIMEOUT_S} s"),
        )),
        Some(StartFail::Exit { code, signal }) => f.push(fail(
            "probe_exit",
            format!("the body exited before `ready` (code {code:?}, signal {signal:?}); stderr is in state/kernel/probe/"),
        )),
        Some(StartFail::Infra(m)) => f.push(fail("probe_exit", format!("the probe could not start: {m}"))),
        None if a.window_exit => f.push(fail(
            "probe_exit",
            format!("the body exited during the {WINDOW_S} s probe window"),
        )),
        None if !a.window_ok => f.push(fail(
            "probe_timeout",
            format!("fewer than two heartbeats in the {WINDOW_S} s probe window"),
        )),
        None => {}
    }
    if a.start_fail.is_none() && !a.window_exit && a.stop_code != Some(0) {
        f.push(fail(
            "probe_no_clean_stop",
            format!(
                "no exit 0 within the grace after `stop` (exit {:?})",
                a.stop_code
            ),
        ));
    }
    let requests = a.u("requests").unwrap_or(0);
    let errors = a.u("errors").unwrap_or(0);
    let rt = a.scope.get("tool_round_trip").and_then(Value::as_bool) == Some(true);
    if a.start_fail.is_none() && (requests == 0 || errors >= requests || !rt) {
        f.push(fail(
            "probe_no_round_trip",
            format!(
                "the proxy saw {requests} request(s), {errors} failed, tool_round_trip {rt}: the probe must send a tool result for a call the provider emitted"
            ),
        ));
    }
    f
}

fn tools_diff(seen: &Value, committed: &[(String, String)]) -> String {
    let seen: Vec<(String, String)> = seen
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| {
                    (
                        t["name"].as_str().unwrap_or("").to_string(),
                        t["schema_sha256"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for (n, h) in &seen {
        match committed.iter().find(|(m, _)| m == n) {
            None => out.push(format!("+{n} (sent, not in self.json)")),
            Some((_, g)) if g != h => out.push(format!("~{n} (schema {h} vs self.json {g})")),
            _ => {}
        }
    }
    for (n, _) in committed {
        if !seen.iter().any(|(m, _)| m == n) {
            out.push(format!("-{n} (in self.json, not sent)"));
        }
    }
    out.join("; ")
}

/// Evaluate G2 (§3.5): every criterion is checked and recorded, the first failure is reported.
pub fn evaluate(i: &G2Input) -> G2Outcome {
    let c = &i.c;
    let p = &i.p;
    let mut warnings = Vec::new();
    let parent_failed = !boot_criteria(p).is_empty();
    let mut failures = boot_criteria(c);
    let booted = c.start_fail.is_none();
    let c_prefix = c.first("prefix_hash").map(str::to_string);
    let p_prefix = p.first("prefix_hash").map(str::to_string);

    // 4. tools_sha256 (proxy-observed) equals G1's.
    if booted
        && let Some(seen) = c.first("tools_sha256")
        && i.self_tools_sha256.as_deref() != Some(seen)
    {
        failures.push(Failure {
            code: "gate.g2.tools_mismatch".into(),
            detail: format!(
                "the proxy saw tools {seen}, self.json says {}: {}",
                i.self_tools_sha256.as_deref().unwrap_or("(none)"),
                tools_diff(
                    c.scope
                        .get("first")
                        .and_then(|f| f.get("tools"))
                        .unwrap_or(&Value::Null),
                    &i.self_tools
                )
            ),
        });
    }
    // 5. prefix hash equals the parent's, unless `Prefix-change: yes`.
    let prefix_equal = c_prefix.is_some() && c_prefix == p_prefix;
    if booted
        && !parent_failed
        && !i.prefix_change
        && c_prefix.is_some()
        && p_prefix.is_some()
        && !prefix_equal
    {
        let offset = i
            .heads
            .as_ref()
            .map(|(a, b)| format!(", head objects first differ at byte {}", first_diff(b, a)))
            .unwrap_or_default();
        failures.push(Failure {
            code: "gate.g2.prefix_hash".into(),
            detail: format!(
                "prefix_hash {} differs from the running version's {} with `Prefix-change: no`{offset}",
                c_prefix.as_deref().unwrap_or(""),
                p_prefix.as_deref().unwrap_or("")
            ),
        });
    }
    if i.prefix_change && prefix_equal {
        warnings.push("prefix_change_declared_but_unchanged".to_string());
    }
    if prefix_equal && c.first("head_bytes_hash") != p.first("head_bytes_hash") {
        warnings.push("head_bytes_changed".to_string());
    }
    // 6. Boot honesty.
    if booted {
        match &c.last_boot_event {
            Some(ev)
                if ev.get("type").and_then(Value::as_str) == Some("boot")
                    && ev.get("sha").and_then(Value::as_str) == Some(c.sha.as_str()) =>
            {
                if let Some(h) = ev.get("prefix_hash").and_then(Value::as_str)
                    && c_prefix.as_deref().is_some_and(|x| x != h)
                {
                    failures.push(Failure {
                        code: "gate.g2.boot_prefix_hash_mismatch".into(),
                        detail: format!(
                            "the boot event claims prefix_hash {h}, the proxy computed {}",
                            c_prefix.as_deref().unwrap_or("")
                        ),
                    });
                }
            }
            Some(ev) => failures.push(Failure {
                code: "gate.g2.boot_event_missing".into(),
                detail: format!(
                    "the last boot event names sha {}, not the candidate",
                    ev.get("sha").and_then(Value::as_str).unwrap_or("(none)")
                ),
            }),
            None => failures.push(Failure {
                code: "gate.g2.boot_event_missing".into(),
                detail: "no boot event in the probe's session log".into(),
            }),
        }
    }
    // 7. Continuity.
    let cont = if i.prefix_change {
        Continuity::Waived
    } else {
        i.continuity.unwrap_or(Continuity::NotApplicable)
    };
    match cont {
        Continuity::Fail { index, offset } if booted => failures.push(Failure {
            code: "gate.g2.continuity".into(),
            detail: format!(
                "the candidate's first request does not continue the running body's last one: element {index} differs at byte {offset}"
            ),
        }),
        Continuity::NotObserved => warnings.push("continuity_not_observed".to_string()),
        _ => {}
    }
    // 8. The probe scope was not stopped by its cap.
    if c.scope.get("capped").and_then(Value::as_bool) == Some(true) {
        failures.push(Failure {
            code: "gate.g2.budget".into(),
            detail: "the probe scope hit its cap (budget.probe_usd)".into(),
        });
    }
    // Warning: TTFT regression (one sample, informational).
    if let (Some(a), Some(b)) = (c.u("ttft_ms_p50"), p.u("ttft_ms_p50"))
        && b > 0
        && a > 2 * b
    {
        warnings.push("ttft_regression".to_string());
    }
    let pass = !parent_failed && failures.is_empty();
    let record = json!({
        "pass": pass,
        "ready_ms": c.ready_ms,
        "parent_ready_ms": p.ready_ms,
        "prefix_hash": c_prefix,
        "parent_prefix_hash": p_prefix,
        "head_bytes_changed": warnings.iter().any(|w| w == "head_bytes_changed"),
        "prefix_change_declared": i.prefix_change,
        "continuity": cont.as_str(),
        "tools_sha256": c.first("tools_sha256"),
        "cache_permille": c.u("cache_permille"),
        "spent_musd": c.u("spent_musd").unwrap_or(0) + p.u("spent_musd").unwrap_or(0),
        "warnings": warnings,
        "failures": failures.iter().map(|f| f.code.clone()).collect::<Vec<_>>(),
    });
    G2Outcome {
        pass,
        parent_failed,
        failures,
        warnings,
        record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(sha: &str, prefix: &str) -> ArmObs {
        ArmObs {
            sha: sha.into(),
            start_fail: None,
            ready_ms: Some(800),
            window_ok: true,
            window_exit: false,
            stop_code: Some(0),
            scope: json!({"requests": 2, "errors": 0, "tool_round_trip": true, "capped": false,
                          "spent_musd": 3, "ttft_ms_p50": 10,
                          "first": {"prefix_hash": prefix, "head_bytes_hash": "hb", "tools_sha256": "sha256:t",
                                    "session": "s1", "tools": [{"name": "bash", "schema_sha256": "sha256:b"}]}}),
            last_boot_event: Some(json!({"type": "boot", "sha": sha, "prefix_hash": prefix})),
        }
    }

    fn input() -> G2Input {
        G2Input {
            c: arm("c", "sha256:P"),
            p: arm("p", "sha256:P"),
            self_tools_sha256: Some("sha256:t".into()),
            self_tools: vec![("bash".into(), "sha256:b".into())],
            prefix_change: false,
            continuity: Some(Continuity::Pass),
            heads: None,
        }
    }

    #[test]
    fn all_criteria_pass() {
        let o = evaluate(&input());
        assert!(o.pass, "{o:?}");
        assert_eq!(o.record["continuity"], "pass");
        assert_eq!(o.record["spent_musd"], 6);
    }

    #[test]
    fn g10_boot_failures() {
        for (sf, code) in [
            (StartFail::Timeout, "gate.g2.probe_timeout"),
            (
                StartFail::Exit {
                    code: Some(101),
                    signal: None,
                },
                "gate.g2.probe_exit",
            ),
        ] {
            let mut i = input();
            i.c.start_fail = Some(sf);
            i.c.scope = Value::Null;
            i.c.last_boot_event = None;
            let o = evaluate(&i);
            assert_eq!(o.code(), Some(code));
            assert_eq!(o.failures.len(), 1, "{o:?}");
        }
        let mut i = input();
        i.c.window_exit = true;
        assert_eq!(evaluate(&i).code(), Some("gate.g2.probe_exit"));
        let mut i = input();
        i.c.stop_code = None;
        assert_eq!(evaluate(&i).code(), Some("gate.g2.probe_no_clean_stop"));
    }

    #[test]
    fn round_trip_tools_prefix_boot_budget() {
        let mut i = input();
        i.c.scope["tool_round_trip"] = json!(false);
        assert_eq!(evaluate(&i).code(), Some("gate.g2.probe_no_round_trip"));

        let mut i = input();
        i.c.scope["first"]["tools_sha256"] = json!("sha256:other");
        i.c.scope["first"]["tools"] = json!([{"name": "bash", "schema_sha256": "sha256:b"}, {"name": "pty", "schema_sha256": "sha256:x"}]);
        let o = evaluate(&i);
        assert_eq!(o.code(), Some("gate.g2.tools_mismatch"));
        assert!(o.detail().contains("+pty"), "{}", o.detail());

        let mut i = input();
        i.c.scope["first"]["prefix_hash"] = json!("sha256:Q");
        i.c.last_boot_event = Some(json!({"type": "boot", "sha": "c"}));
        i.heads = Some((
            b"{\"system\":\"a1\"}".to_vec(),
            b"{\"system\":\"a2\"}".to_vec(),
        ));
        let o = evaluate(&i);
        assert_eq!(o.code(), Some("gate.g2.prefix_hash"));
        assert!(o.detail().contains("byte 12"), "{}", o.detail());
        i.prefix_change = true;
        assert!(evaluate(&i).pass);

        let mut i = input();
        i.c.last_boot_event =
            Some(json!({"type": "boot", "sha": "c", "prefix_hash": "sha256:fake"}));
        assert_eq!(
            evaluate(&i).code(),
            Some("gate.g2.boot_prefix_hash_mismatch")
        );
        i.c.last_boot_event = Some(json!({"type": "boot", "sha": "p"}));
        assert_eq!(evaluate(&i).code(), Some("gate.g2.boot_event_missing"));

        let mut i = input();
        i.c.scope["capped"] = json!(true);
        assert_eq!(evaluate(&i).code(), Some("gate.g2.budget"));
    }

    #[test]
    fn parent_failure_is_infra() {
        let mut i = input();
        i.p.start_fail = Some(StartFail::Timeout);
        let o = evaluate(&i);
        assert!(!o.pass && o.parent_failed);
        assert_eq!(o.code(), Some("gate.g2.parent_probe"));
    }

    #[test]
    fn warnings() {
        let mut i = input();
        i.prefix_change = true;
        i.c.scope["first"]["head_bytes_hash"] = json!("hb2");
        i.c.scope["ttft_ms_p50"] = json!(50);
        let o = evaluate(&i);
        assert!(o.pass);
        for w in [
            "prefix_change_declared_but_unchanged",
            "head_bytes_changed",
            "ttft_regression",
        ] {
            assert!(o.warnings.iter().any(|x| x == w), "{w}: {:?}", o.warnings);
        }
        assert_eq!(o.record["continuity"], "waived");
    }

    #[test]
    fn g13_continuity() {
        let el = |s: &str| s.as_bytes().to_vec();
        let rp = vec![
            el(r#"{"role":"system","content":"S"}"#),
            el(r#"{"content":"out: 1","role":"tool","tool_call_id":"a"}"#),
        ];
        // Key order and whitespace do not matter: elements are compared re-serialized.
        let same = vec![
            el(r#"{"content": "S", "role": "system"}"#),
            el(r#"{"role":"tool","tool_call_id":"a","content":"out: 1"}"#),
            el(r#"{"role":"user","content":"[init:probe]"}"#),
        ];
        assert_eq!(continuity(Some(&rp), &[same]), Continuity::Pass);
        let changed = vec![
            el(r#"{"role":"system","content":"S"}"#),
            el(r#"{"role":"tool","tool_call_id":"a","content":"output: 1"}"#),
            el("{}"),
        ];
        let short = vec![el(r#"{"role":"system","content":"S"}"#)];
        match continuity(Some(&rp), &[short.clone(), changed]) {
            Continuity::Fail { index: 1, offset } => assert!(offset > 0),
            other => panic!("{other:?}"),
        }
        assert_eq!(continuity(Some(&rp), &[short]), Continuity::NotObserved);
        assert_eq!(continuity(None, &[]), Continuity::NotApplicable);
        let mut i = input();
        i.continuity = Some(Continuity::Fail {
            index: 3,
            offset: 9,
        });
        let o = evaluate(&i);
        assert_eq!(o.code(), Some("gate.g2.continuity"));
        assert!(o.detail().contains("element 3"));
    }
}
