//! `GET /_init/*` (kernel/proxy.md §4.3): stats, health, models and the agent's trace view (§3.14).
//! Everything here is ICJ: sorted keys, integers only.

use crate::money::pusd_to_musd;
use crate::request::valid_id;
use crate::scope::Scope;
use crate::server::{Shared, full, json_resp};
use crate::time::{now_ms, rfc3339_ms, rfc3339_s};
use crate::trace::{self, SummaryQuery};
use crate::types::{ScopeKind, Visibility};
use hyper::Response;
use hyper::http::HeaderValue;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::Ordering;

type Resp = Response<crate::server::ProxyBody>;

fn not_found(code: &str) -> Resp {
    let e = crate::errors::ProxyError::new(404, code, "not found");
    let (s, h, b) = e.render(false);
    let mut r = Response::new(full(b));
    *r.status_mut() = hyper::StatusCode::from_u16(s).unwrap_or(hyper::StatusCode::NOT_FOUND);
    for (k, v) in h {
        if let Ok(v) = HeaderValue::from_str(&v) {
            r.headers_mut().insert(k, v);
        }
    }
    r
}

fn query_param<'a>(q: Option<&'a str>, key: &str) -> Option<&'a str> {
    q?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })
}

/// Answer `GET /_init/<rest>`.
pub fn local(
    st: &Arc<Shared>,
    scope: &Arc<Scope>,
    method: &str,
    rest: &str,
    query: Option<&str>,
) -> Resp {
    if method != "GET" {
        return not_found("route.unknown");
    }
    match rest {
        "stats" => json_resp(200, &stats(st, scope), "application/json"),
        "health" => json_resp(200, &health(st), "application/json"),
        "models" => json_resp(200, &models(st), "application/json"),
        _ if rest == "traces" || rest.starts_with("traces/") => traces(st, scope, rest, query),
        _ => not_found("route.unknown"),
    }
}

fn permille(read: u64, total: u64) -> Option<u64> {
    (total > 0).then(|| read * 1000 / total)
}

/// `GET /_init/stats` for the socket's scope (§4.3).
pub fn stats(st: &Shared, scope: &Scope) -> Value {
    let now = now_ms();
    let cfg = st.cfg();
    let level = st.level();
    let s = scope.state.lock().map(|s| s.clone()).unwrap_or_default();
    let a = st.acct.lock().map(|a| a.clone());
    let Ok(a) = a else { return json!({"v": 1}) };
    let by_model: Map<String, Value> = a
        .by_model
        .iter()
        .map(|(k, (n, spent))| {
            (
                k.clone(),
                json!({"requests": n, "today_musd": pusd_to_musd(*spent)}),
            )
        })
        .collect();
    let task = match (&scope.kind, &s.last_task) {
        (ScopeKind::Live | ScopeKind::Member(_), t) => {
            let id = t.clone().unwrap_or_else(|| "_none".into());
            let key = format!("{}\0{}", scope.id, id);
            let spent = a.tasks.get(&key).map(|c| c.spent).unwrap_or(0);
            let tbm: Map<String, Value> = a
                .by_task_model
                .iter()
                .filter(|((tk, _), _)| *tk == key)
                .map(|((_, m), v)| (m.clone(), json!(pusd_to_musd(*v))))
                .collect();
            t.as_ref().map(|_| {
                json!({"id": id, "today_musd": pusd_to_musd(spent), "cap_musd": a.configured.task_musd, "by_model": tbm})
            })
        }
        _ => None,
    };
    let cache_last = s.cache.back().and_then(|x| permille(x.1, x.2));
    let (tr, tt) = s
        .cache
        .iter()
        .filter(|x| x.0 >= a.day_start_ms)
        .fold((0, 0), |acc, x| (acc.0 + x.1, acc.1 + x.2));
    let (hr, ht) = s
        .cache
        .iter()
        .filter(|x| x.0 + 3_600_000 >= now)
        .fold((0, 0), |acc, x| (acc.0 + x.1, acc.1 + x.2));
    let session = s.last_session.clone().unwrap_or_else(|| "_none".into());
    let session_permille = s
        .cache_by_session
        .get(&session)
        .and_then(|(r, t)| permille(*r, *t));
    let hour = now.saturating_sub(3_600_000);
    let providers: Map<String, Value> = {
        let ps = st.providers.lock().map(|p| p.clone()).unwrap_or_default();
        cfg.providers
            .iter()
            .map(|(id, p)| {
                let x = ps.get(id).cloned().unwrap_or_default();
                (
                    id.clone(),
                    json!({"enabled": p.enabled, "requests": x.requests, "errors_upstream": x.errors_upstream,
                           "errors_proxy": x.errors_proxy, "retries": x.retries, "ws": null, "balance": null}),
                )
            })
            .collect()
    };
    let connects = st.egress.connects.load(Ordering::Relaxed);
    let requests = st.egress.requests.load(Ordering::Relaxed);
    let label = scope.label();
    json!({
        "v": 1,
        "ts": rfc3339_ms(now),
        "spend": {
            "enforced": a.money_enforced(),
            "policy": if a.money_enforced() { "limit" } else { "observe" },
            "today_musd": pusd_to_musd(a.day.spent),
            "cap_today_musd": pusd_to_musd(a.eff_day()),
            "reserved_musd": pusd_to_musd(a.day.reserved),
            "day_starts_at": (a.day_start_ms > 0).then(|| rfc3339_s(a.day_start_ms)),
            "resets_at": (a.day_end_ms > 0).then(|| rfc3339_s(a.day_end_ms)),
            "evolution_today_musd": pusd_to_musd(a.evolution.spent),
            "evolution_cap_musd": pusd_to_musd(a.evolution_cap()),
            "budgets": {"evolution_share_permille": a.configured.evolution_share_permille, "gate_musd": a.configured.gate_musd,
                        "task_musd": a.configured.task_musd, "source": a.source},
            "by_model": by_model,
            "task": task,
        },
        "routing": {"level": level.as_str(), "default": format!("{}/{}", cfg.default_provider, cfg.default_model), "routed_today": a.routed_today},
        "root": a.root.as_ref().map(|r| r.ceilings.clone()),
        "cache": {"last_permille": cache_last, "session_permille": session_permille,
                  "today_permille": permille(tr, tt), "window_1h_permille": permille(hr, ht)},
        "last_head_hash": s.last_head_hash,
        "requests_today": a.requests_today,
        "scope": {"id": scope.id, "kind": scope.kind.as_str(), "job_id": scope.job_id, "label": label.to_json(),
                  "opened": rfc3339_ms(scope.opened_ms), "requests": s.requests, "inflight": s.inflight, "errors": s.errors,
                  "retries": s.retries, "spent_musd": pusd_to_musd(s.spent_pusd), "cap_musd": scope.cap_musd},
        "last": s.last,
        "latency_1h": {"requests": s.ttft.count_since(hour), "ttft_ms_p50": s.ttft.pct(50, hour), "ttft_ms_p90": s.ttft.pct(90, hour),
                       "tps_p50": s.tps.pct(50, hour), "gap_ms_p50": s.gap.pct(50, hour)},
        "egress": {"connects": connects, "requests": requests,
                   "reuse_permille": (requests > 0).then(|| requests.saturating_sub(connects.min(requests)) * 1000 / requests),
                   "root_refusals": a.root_refusals, "cost_mismatch": st.cost_mismatch.load(Ordering::Relaxed)},
        "providers": providers,
        "trace_dropped": st.writer.dropped(),
    })
}

/// `GET /_init/health`.
pub fn health(st: &Shared) -> Value {
    let cfg = st.cfg();
    let root = st.root();
    let ok_ms = st.root_ok_ms.load(Ordering::Relaxed);
    let stale = ok_ms == 0 || now_ms().saturating_sub(ok_ms) > 60_000;
    let providers: Map<String, Value> = cfg
        .providers
        .iter()
        .map(|(id, p)| (id.clone(), json!({"enabled": p.enabled, "forwarded": root.as_ref().is_some_and(|r| r.forwards_provider(id))})))
        .collect();
    json!({"v": 1, "ok": root.is_some() && !stale,
           "egress": {"connected": st.egress_down_since.lock().map(|d| d.is_none()).unwrap_or(false) && root.is_some(),
                      "root_state_at": (ok_ms > 0).then(|| rfc3339_ms(ok_ms))},
           "root_state_stale": stale,
           "level": st.level().as_str(), "providers": providers, "test_mode": cfg.raw.proxy.test_mode})
}

/// `GET /_init/models`: the routing view (§3.5a).
pub fn models(st: &Shared) -> Value {
    let cfg = st.cfg();
    let root = st.root();
    let level = st.level();
    let set = st.routable_set(&cfg, root.as_ref(), level);
    let routable: Vec<Value> = set
        .iter()
        .filter_map(|(p, m)| {
            let t = cfg.model(p, m)?;
            let prices: Map<String, Value> = t
                .prices
                .iter()
                .map(|(w, x)| {
                    (w.clone(), json!({"input_musd_per_mtok": x.input, "cache_read_musd_per_mtok": x.cache_read,
                                       "cache_write_musd_per_mtok": x.cache_write, "cache_write_1h_musd_per_mtok": x.cache_write_1h,
                                       "output_musd_per_mtok": x.output}))
                })
                .collect();
            Some(json!({"id": format!("{p}/{m}"), "prices": prices, "reference_window": t.reference_window}))
        })
        .collect();
    json!({"v": 1, "level": level.as_str(), "default": format!("{}/{}", cfg.default_provider, cfg.default_model), "routable": routable})
}

/// The agent's redacted read-only view (§3.14): only on `run/proxy.sock`, only `vis: agent`.
fn traces(st: &Shared, scope: &Scope, rest: &str, query: Option<&str>) -> Resp {
    if scope.kind != ScopeKind::Live || st.judge {
        return not_found("traces.not_found");
    }
    let root = &st.traces_root;
    st.writer.flush();
    if rest == "traces" {
        let limit = query_param(query, "limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100)
            .clamp(1, 500);
        let after = query_param(query, "after")
            .filter(|a| a.len() == 16 && a.bytes().all(|c| c.is_ascii_hexdigit()));
        let task = query_param(query, "task").filter(|t| valid_id(t));
        let q = SummaryQuery {
            after: after.map(str::to_string),
            limit,
            task: task.map(str::to_string),
            vis: Some(Visibility::Agent.as_str().to_string()),
        };
        let lines = trace::summaries(root, &q).unwrap_or_default();
        let mut body = Vec::new();
        for l in lines {
            body.extend(serde_json::to_vec(&l).unwrap_or_default());
            body.push(b'\n');
        }
        let mut r = Response::new(full(body));
        r.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("application/x-ndjson"),
        );
        return r;
    }
    let parts: Vec<&str> = rest.split('/').collect();
    let (id, part) = match parts.as_slice() {
        ["traces", id, part] => (*id, *part),
        _ => return not_found("traces.not_found"),
    };
    if id.len() != 16
        || !id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return not_found("traces.not_found");
    }
    let Ok((start, end)) = trace::records(root, id) else {
        return not_found("traces.not_found");
    };
    if start.get("vis").and_then(Value::as_str) != Some("agent") {
        return not_found("traces.not_found");
    }
    match part {
        "request" => match trace::reconstruct(root, id) {
            Ok((bytes, _)) => {
                let mut r = Response::new(full(bytes));
                r.headers_mut()
                    .insert("content-type", HeaderValue::from_static("application/json"));
                r
            }
            Err(_) => not_found("traces.not_found"),
        },
        "response" => match trace::response(root, id) {
            Ok(bytes) => {
                let ct = end
                    .as_ref()
                    .and_then(|e| e.get("resp_type"))
                    .and_then(Value::as_str)
                    .and_then(|c| HeaderValue::from_str(c).ok())
                    .unwrap_or(HeaderValue::from_static("application/octet-stream"));
                let mut r = Response::new(full(bytes));
                r.headers_mut().insert("content-type", ct);
                r
            }
            Err(_) => not_found("traces.not_found"),
        },
        _ => not_found("traces.not_found"),
    }
}
