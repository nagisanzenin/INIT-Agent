//! Scopes (lanes), their counters and the observations the gate reads (kernel/proxy.md §3.4, §3.11,
//! §3.12, §4.4 `ScopeSummary`).

use crate::money::pusd_to_musd;
use crate::tap::{Family, Usage};
use crate::time::rfc3339_ms;
use crate::types::{Label, ScopeKind, Visibility};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

/// Per-model use within a scope (§3.12 `ModelUse`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelUse {
    pub requests: u64,
    pub routed: u64,
    pub spent_pusd: u64,
    pub cost_ref_pusd: u64,
}

/// The first metered request of a scope (§3.12 `RequestObs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestObs {
    pub req_id: String,
    pub model: String,
    pub family: Family,
    pub prefix_hash: String,
    pub head_bytes_hash: String,
    pub tools_sha256: String,
    pub session: Option<String>,
    /// `(name, schema_sha256)` in request order.
    pub tools: Vec<(String, String)>,
}

/// §3.12 `Observations`.
#[derive(Clone, Debug, Default)]
pub struct Observations {
    pub first: Option<RequestObs>,
    pub prefix_hashes: Vec<(String, u64)>,
    pub tools_sha256: Vec<(String, u64)>,
    pub models: BTreeSet<String>,
    pub by_model: BTreeMap<String, ModelUse>,
    pub last_req_by_session: BTreeMap<String, String>,
    pub emitted_call_ids: VecDeque<String>,
    pub tool_round_trip: bool,
    pub tool_calls: BTreeMap<String, u64>,
    /// Normalized finish reason and Unix ms.
    pub last_finish: Option<(String, u64)>,
}

fn bump(v: &mut Vec<(String, u64)>, k: &str) {
    match v.iter_mut().find(|x| x.0 == k) {
        Some(x) => x.1 += 1,
        None => v.push((k.to_string(), 1)),
    }
}

impl Observations {
    /// A metered request was admitted.
    pub fn on_request(&mut self, obs: RequestObs, tool_result_ids: &[String]) {
        bump(&mut self.prefix_hashes, &obs.prefix_hash);
        bump(&mut self.tools_sha256, &obs.tools_sha256);
        self.models.insert(obs.model.clone());
        if let Some(s) = &obs.session {
            self.last_req_by_session
                .insert(s.clone(), obs.req_id.clone());
        }
        if !self.tool_round_trip
            && tool_result_ids
                .iter()
                .any(|id| self.emitted_call_ids.contains(id))
        {
            self.tool_round_trip = true;
        }
        if self.first.is_none() {
            self.first = Some(obs);
        }
    }

    /// The response's tool calls (trusted: from the provider stream).
    pub fn on_tool_calls(&mut self, calls: &[crate::tap::ToolCall]) {
        for c in calls {
            if !c.id.is_empty() {
                if self.emitted_call_ids.len() >= 1024 {
                    self.emitted_call_ids.pop_front();
                }
                self.emitted_call_ids.push_back(c.id.clone());
            }
            *self
                .tool_calls
                .entry(format!("tool/{}", c.name))
                .or_default() += 1;
            if let Some(e) = &c.ext_name {
                *self.tool_calls.entry(format!("ext/{e}")).or_default() += 1;
            }
        }
    }
}

/// A bounded ring of `(unix ms, value)` samples.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    pub cap: usize,
    pub v: VecDeque<(u64, u64)>,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring {
            cap: cap.max(1),
            v: VecDeque::new(),
        }
    }
    pub fn push(&mut self, ms: u64, x: u64) {
        if self.v.len() >= self.cap {
            self.v.pop_front();
        }
        self.v.push_back((ms, x));
    }
    /// The `p`-th percentile (0..=100) of the samples at or after `since_ms`, by sorting a copy.
    pub fn pct(&self, p: u64, since_ms: u64) -> Option<u64> {
        let mut xs: Vec<u64> = self
            .v
            .iter()
            .filter(|(t, _)| *t >= since_ms)
            .map(|x| x.1)
            .collect();
        if xs.is_empty() {
            return None;
        }
        xs.sort_unstable();
        let i = ((xs.len() - 1) as u64 * p / 100) as usize;
        Some(xs[i])
    }
    pub fn count_since(&self, since_ms: u64) -> u64 {
        self.v.iter().filter(|(t, _)| *t >= since_ms).count() as u64
    }
}

/// Per-run caps on actuals (m7-design.md §3.1.3, §3.2.8; kernel/gate.md §3.11): set on a gate `run`
/// lane by [`crate::ProxyHandle::set_run_caps`]. Unlike the money cap they use no reservations: after
/// each response the lane adds the observed usage, and once a counter has reached its cap the next
/// request is refused (429, `x-init-cap: turns|tokens_out|tokens_in`). The first request is therefore
/// never refused by the token caps, whatever it would reserve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunCaps {
    /// Requests counted as turns (`x-init-purpose: turn`, or no purpose header: absence cannot
    /// bypass the cap). `probe` and `compaction` requests are not turns.
    pub max_turns: Option<u64>,
    /// Output tokens, reasoning included, summed over the run's responses.
    pub max_output_tokens: Option<u64>,
    /// Input tokens, cached included, summed over the run's responses.
    pub max_input_tokens: Option<u64>,
}

/// The run-cap counters of one lane, updated synchronously when a response ends (before the body can
/// send its next request), so the refusal is exact on actuals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapState {
    pub caps: RunCaps,
    pub turns: u64,
    pub tokens_out: u64,
    pub tokens_in: u64,
}

impl CapState {
    /// The cap that refuses the next request, in the order turns, tokens_out, tokens_in:
    /// `(cap name, limit, counter)`.
    pub fn refusal(&self) -> Option<(&'static str, u64, u64)> {
        let c = &self.caps;
        [
            ("turns", c.max_turns, self.turns),
            ("tokens_out", c.max_output_tokens, self.tokens_out),
            ("tokens_in", c.max_input_tokens, self.tokens_in),
        ]
        .into_iter()
        .find_map(|(n, lim, v)| lim.filter(|l| v >= *l).map(|l| (n, l, v)))
    }
}

/// Whether a request with this `x-init-purpose` counts as a turn for the cap.
pub fn counts_as_turn(purpose: Option<&str>) -> bool {
    !matches!(purpose, Some("probe") | Some("compaction"))
}

/// Mutable per-scope state.
#[derive(Clone, Debug, Default)]
pub struct ScopeState {
    pub requests: u64,
    pub inflight: u32,
    pub errors: u64,
    pub retries: u64,
    pub spent_pusd: u64,
    pub cost_ref_pusd: u64,
    pub usage: Usage,
    pub closed_ms: Option<u64>,
    pub capped: bool,
    pub obs: Observations,
    pub ttft: Ring,
    pub ttfc: Ring,
    pub tps: Ring,
    pub gap: Ring,
    /// `(unix ms, cache read tokens, input tokens)` of requests with usage.
    pub cache: VecDeque<(u64, u64, u64)>,
    pub cache_by_session: BTreeMap<String, (u64, u64)>,
    pub last: Option<Value>,
    pub last_head_hash: Option<String>,
    pub last_session: Option<String>,
    pub last_task: Option<String>,
    /// Per `x-init-session`: when the previous response ended and whether it ended with tool calls.
    pub prev_end: BTreeMap<String, (u64, bool)>,
    /// Per-run caps and their counters (`None`: the lane has no run caps).
    pub run: Option<CapState>,
    /// The first cap that refused a request on this lane: `turns`, `tokens_out`, `tokens_in`, or
    /// `cost` for a money cap (RunRecord v3 `caps_hit`).
    pub caps_hit: Option<&'static str>,
    /// The last served model string a response named (RunRecord v3 `model_served`).
    pub model_served: Option<String>,
}

/// One lane (§3.4).
#[derive(Debug)]
pub struct Scope {
    pub id: String,
    pub kind: ScopeKind,
    pub job_id: Option<String>,
    pub visibility: Visibility,
    pub cap_musd: Option<u64>,
    pub label: Mutex<Label>,
    /// `x-egress-body`: the live body's boot id, or the probe id of a gate lane.
    pub body_id: Mutex<String>,
    pub socket: Option<PathBuf>,
    pub opened_ms: u64,
    pub state: Mutex<ScopeState>,
}

impl Scope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        kind: ScopeKind,
        job_id: Option<String>,
        visibility: Visibility,
        cap_musd: Option<u64>,
        label: Label,
        socket: Option<PathBuf>,
        ring: usize,
        now_ms: u64,
    ) -> Scope {
        let body_id = socket
            .as_ref()
            .and_then(|p| {
                // probe/<probe_id>/run/proxy.sock → <probe_id>
                let parts: Vec<_> = p
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                let n = parts.len();
                (n >= 3 && parts[n - 2] == "run" && kind.is_gate()).then(|| parts[n - 3].clone())
            })
            .unwrap_or_default();
        Scope {
            id,
            kind,
            job_id,
            visibility,
            cap_musd,
            label: Mutex::new(label),
            body_id: Mutex::new(body_id),
            socket,
            opened_ms: now_ms,
            state: Mutex::new(ScopeState {
                ttft: Ring::new(ring),
                ttfc: Ring::new(ring),
                tps: Ring::new(ring),
                gap: Ring::new(ring),
                ..ScopeState::default()
            }),
        }
    }

    pub fn label(&self) -> Label {
        self.label.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// `ScopeSummary` (§4.4), ICJ integers only.
    pub fn summary(&self) -> Value {
        let st = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        let o = &st.obs;
        let first = o.first.as_ref().map(|f| {
            json!({"req_id": f.req_id, "model": f.model, "family": f.family.as_str(), "session": f.session,
                   "prefix_hash": f.prefix_hash, "head_bytes_hash": f.head_bytes_hash, "tools_sha256": f.tools_sha256,
                   "tools": f.tools.iter().map(|(n, s)| json!({"name": n, "schema_sha256": s})).collect::<Vec<_>>()})
        });
        let by_model: serde_json::Map<String, Value> = o
            .by_model
            .iter()
            .map(|(k, m)| {
                (k.clone(), json!({"cost_ref_pusd": m.cost_ref_pusd, "requests": m.requests, "routed": m.routed, "spent_pusd": m.spent_pusd}))
            })
            .collect();
        let (read, total) = st
            .cache
            .iter()
            .fold((0u64, 0u64), |a, x| (a.0 + x.1, a.1 + x.2));
        json!({
            "scope_id": self.id, "kind": self.kind.as_str(), "job_id": self.job_id, "visibility": self.visibility.as_str(),
            "opened": rfc3339_ms(self.opened_ms), "closed": st.closed_ms.map(rfc3339_ms),
            "requests": st.requests, "inflight": st.inflight, "errors": st.errors,
            "spent_pusd": st.spent_pusd, "cost_ref_pusd": st.cost_ref_pusd,
            "usage": serde_json::to_value(st.usage).unwrap_or(Value::Null),
            "cache_permille": (total > 0).then(|| read * 1000 / total),
            "ttft_ms_p50": st.ttft.pct(50, 0), "tps_p50": st.tps.pct(50, 0), "gap_ms_p50": st.gap.pct(50, 0),
            "first": first,
            "prefix_hashes": o.prefix_hashes.iter().map(|(h, n)| json!([h, n])).collect::<Vec<_>>(),
            "tools_sha256s": o.tools_sha256.iter().map(|(h, n)| json!([h, n])).collect::<Vec<_>>(),
            "models": o.models.iter().collect::<Vec<_>>(),
            "by_model": Value::Object(by_model),
            "tool_round_trip": o.tool_round_trip,
            "tool_calls": o.tool_calls,
            "last_req_by_session": o.last_req_by_session,
            "last_finish": o.last_finish.as_ref().map(|(r, t)| json!({"reason": r, "at": rfc3339_ms(*t)})),
            "capped": st.capped,
            "spent_musd": pusd_to_musd(st.spent_pusd),
            "turns": st.run.as_ref().map(|r| r.turns),
            "caps": st.run.as_ref().map(|r| json!({"max_turns": r.caps.max_turns,
                "max_output_tokens": r.caps.max_output_tokens, "max_input_tokens": r.caps.max_input_tokens})),
            "caps_hit": st.caps_hit,
            "model_served": st.model_served,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tap::ToolCall;

    /// P-14: a fabricated call id never sets `tool_round_trip`; an emitted one does.
    #[test]
    fn p14_tool_round_trip() {
        let mut o = Observations::default();
        let obs = |id: &str| RequestObs {
            req_id: id.into(),
            model: "m".into(),
            family: Family::OpenAiChat,
            prefix_hash: "p".into(),
            head_bytes_hash: "h".into(),
            tools_sha256: "t".into(),
            session: Some("s".into()),
            tools: vec![],
        };
        o.on_request(obs("1"), &["call_fake".into()]);
        assert!(!o.tool_round_trip);
        o.on_tool_calls(&[ToolCall {
            id: "call_00_x".into(),
            name: "bash".into(),
            args: "{}".into(),
            ext_name: None,
        }]);
        o.on_request(obs("2"), &["call_fake".into()]);
        assert!(!o.tool_round_trip);
        o.on_request(obs("3"), &["call_00_x".into()]);
        assert!(o.tool_round_trip);
        assert_eq!(o.prefix_hashes, vec![("p".to_string(), 3)]);
        assert_eq!(o.first.as_ref().unwrap().req_id, "1");
        assert_eq!(o.tool_calls["tool/bash"], 1);
    }

    /// M7-G07 (unit): caps refuse only once a counter has reached its cap, in the order turns,
    /// tokens_out, tokens_in; a fresh lane is never refused.
    #[test]
    fn m7_g07_cap_state_refusal() {
        let caps = RunCaps { max_turns: Some(12), max_output_tokens: Some(24_000), max_input_tokens: Some(400_000) };
        let mut c = CapState { caps, ..CapState::default() };
        assert_eq!(c.refusal(), None);
        c.turns = 11;
        c.tokens_out = 23_999;
        assert_eq!(c.refusal(), None);
        c.turns = 12;
        assert_eq!(c.refusal(), Some(("turns", 12, 12)));
        c.turns = 3;
        c.tokens_out = 24_000;
        c.tokens_in = 500_000;
        assert_eq!(c.refusal(), Some(("tokens_out", 24_000, 24_000)));
        c.tokens_out = 0;
        assert_eq!(c.refusal(), Some(("tokens_in", 400_000, 500_000)));
        assert_eq!(CapState::default().refusal(), None);
        assert!(counts_as_turn(Some("turn")) && counts_as_turn(None));
        assert!(!counts_as_turn(Some("probe")) && !counts_as_turn(Some("compaction")));
    }

    #[test]
    fn ring_percentiles() {
        let mut r = Ring::new(4);
        for (t, x) in [(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)] {
            r.push(t, x);
        }
        assert_eq!(r.v.len(), 4);
        assert_eq!(r.pct(50, 0), Some(30));
        assert_eq!(r.pct(90, 4), Some(40));
        assert_eq!(r.count_since(4), 2);
    }

    #[test]
    fn probe_id_from_socket() {
        let s = Scope::new(
            "j-000001/probe-c".into(),
            ScopeKind::Probe,
            Some("j-000001".into()),
            Visibility::Agent,
            Some(1),
            Label::default(),
            Some(PathBuf::from("/h/probe/0123456789ab/run/proxy.sock")),
            8,
            0,
        );
        assert_eq!(*s.body_id.lock().unwrap(), "0123456789ab");
        assert_eq!(s.summary()["kind"], "probe");
    }
}
