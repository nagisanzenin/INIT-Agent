//! The Root status as the awareness texts read it (awareness.md §4.5, whose field names are
//! placeholders until root/root.md fixes the shapes). The kernel copies the Root's real objects
//! into `boot.json.env.root` (kernel-supervise: `{level, max_level, next, ceilings, classes,
//! audited_good}`, with `ceilings` = the Root's `ceilings.json` view: `money.day_limit_musd`, …,
//! `parallel_bodies.limit`, and `classes` = objects with `id`, `enabled`, `min_level`). This module
//! fills the §4.5 names from those shapes and, per awareness §4.5 / root.md X2, from the Root's
//! published view `INIT_HOME/root/{ledger,ceilings,charter}.json` when the body may read it. The
//! kernel sends no `cap`: it comes from `ledger.json` (`cap`), else min(`max_level`,
//! `supported_max_level`) from `ledger.json` or `charter.json` (`autonomy.max_level`,
//! `supported_max_level`, root.md §3.12.5); with none of them readable it stays absent and the
//! autonomy line leaves it out. It also fills
//! `selfmap.body_tokens` from the committed `body/self.json` (`budget.tokens`, the same o200k
//! measure) when the kernel sent none.
//!
//! The result is used only for rendering (boot block, signal context). The boot event keeps
//! `boot.json` verbatim in `facts.kernel` and `env.kernel` (awareness §3.9: copied, never edited).

use std::path::Path;

use serde_json::{Map, Value, json};

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn u(v: &Value, path: &[&str]) -> Option<u64> {
    let mut c = v;
    for k in path {
        c = c.get(k)?;
    }
    c.as_u64()
}

fn set_if_missing(m: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    if m.get(k).is_none_or(Value::is_null)
        && let Some(v) = v.filter(|v| !v.is_null())
    {
        m.insert(k.to_string(), v);
    }
}

/// `env.root` with the §4.5 names filled in. `ledger`/`ceil`/`charter` are the Root's published
/// views (`INIT_HOME/root/*.json`).
pub fn normalize_root(
    root: &Value,
    ledger: Option<&Value>,
    ceil: Option<&Value>,
    charter: Option<&Value>,
) -> Value {
    let mut r = root.as_object().cloned().unwrap_or_default();
    if let Some(l) = ledger {
        for k in [
            "level",
            "cap",
            "max_level",
            "supported_max_level",
            "next",
            "last_change",
        ] {
            set_if_missing(&mut r, k, l.get(k).cloned());
        }
    }
    if let Some(c) = charter {
        set_if_missing(
            &mut r,
            "max_level",
            u(c, &["autonomy", "max_level"]).map(Value::from),
        );
        set_if_missing(
            &mut r,
            "supported_max_level",
            u(c, &["supported_max_level"]).map(Value::from),
        );
    }
    let level = r.get("level").and_then(Value::as_u64);
    // cap = min(charter max, the Root release's supported max) when not given.
    if r.get("cap").is_none_or(Value::is_null)
        && let (Some(m), Some(s)) = (
            r.get("max_level").and_then(Value::as_u64),
            r.get("supported_max_level").and_then(Value::as_u64),
        )
    {
        r.insert("cap".into(), m.min(s).into());
    }
    // Ceilings: the §4.5 names from the Root's money/parallel_bodies shapes.
    let src = r
        .get("ceilings")
        .filter(|c| c.is_object())
        .cloned()
        .or_else(|| ceil.cloned())
        .unwrap_or(Value::Null);
    let mut c = src.as_object().cloned().unwrap_or_default();
    let alt = |path: &[&str]| -> Option<Value> {
        u(&src, path)
            .or_else(|| ceil.and_then(|x| u(x, path)))
            .map(Value::from)
    };
    set_if_missing(&mut c, "day_musd", alt(&["money", "day_limit_musd"]));
    set_if_missing(&mut c, "total_musd", alt(&["money", "total_limit_musd"]));
    set_if_missing(
        &mut c,
        "total_used_musd",
        alt(&["money", "total_spent_musd"]),
    );
    if let Some(n) = c
        .get("parallel_bodies")
        .and_then(|p| p.get("limit"))
        .cloned()
    {
        c.insert("parallel_bodies".into(), n);
    }
    set_if_missing(
        &mut c,
        "parallel_bodies",
        alt(&["parallel_bodies", "limit"]),
    );
    if !c.is_empty() {
        r.insert("ceilings".into(), Value::Object(c));
    }
    // Classes: sorted ids of the enabled classes allowed at this level.
    let classes = r
        .get("classes")
        .filter(|c| c.as_array().is_some_and(|a| !a.is_empty()))
        .cloned()
        .or_else(|| src.get("classes").cloned())
        .or_else(|| ceil.and_then(|x| x.get("classes").cloned()));
    if let Some(Value::Array(a)) = classes {
        let mut ids: Vec<String> = a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => {
                    let enabled = o.get("enabled").and_then(Value::as_bool) != Some(false);
                    let allowed = match (o.get("min_level").and_then(Value::as_u64), level) {
                        (Some(m), Some(l)) => m <= l,
                        _ => true,
                    };
                    (enabled && allowed)
                        .then(|| o.get("id").and_then(Value::as_str).map(str::to_string))
                        .flatten()
                }
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        r.insert(
            "classes".into(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
    }
    Value::Object(r)
}

/// ADR-011 defaults (§4.5 open groups and bloat factor by level L0…L4, §9 window turns, §6.4 caps
/// at L0). The Root's charter view leaves `[extend]` out exactly at these defaults.
const OPEN_GROUPS: [u64; 5] = [4, 6, 8, 12, 12];
const BLOAT_PERMILLE: [u64; 5] = [1000, 1500, 2000, 3000, 3000];
const WIN_TURNS: u64 = 20;
/// A kernel group's window (§9; a Root constant, not a charter key).
const KERNEL_WIN_TURNS: u64 = 40;
const BLOAT_CAPS: u64 = 64;
/// Cap states that are still live (the Root's X6 `select.rs` `LIVE`).
const LIVE: [&str; 4] = ["active", "cold", "archive_due", "quarantined"];

fn ids(v: Option<&Value>) -> Option<Vec<Value>> {
    let a = v?.as_array()?;
    let mut out: Vec<String> = a
        .iter()
        .filter_map(|x| x.as_str().or_else(|| x.get("cap")?.as_str()))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    Some(out.into_iter().map(Value::String).collect())
}

/// ADR-011 X11: the free path's status for the boot block's `extend` line, folded from the Root's
/// published views: `root/extend.json` (X3: open groups with their window turns, the H5a freeze, a
/// pending rollback; X6: `archive_due`, `quarantine_debt`), `root/caps.json` (X5: the cap table)
/// and `charter.json` `extend` (absent at its defaults; an unreadable charter leaves the limits
/// unknown). `mode` is the Root's `ceilings.extend.mode` when known. None on the gated path: no
/// `extend` mode and no open group.
pub fn normalize_extend(
    ext: Option<&Value>,
    caps: Option<&Value>,
    mode: Option<&str>,
    charter: Option<&Value>,
    level: Option<u64>,
) -> Option<Value> {
    // Some(None): a charter view without `[extend]`, so every value is its default.
    let sect = charter.map(|c| c.get("extend").filter(|e| e.is_object()));
    let mode = mode
        .or_else(|| sect.flatten().and_then(|e| e.get("mode")?.as_str()))
        .or_else(|| sect.and_then(|s| s.is_none().then_some("gated")));
    let groups: Vec<&Value> = ext
        .and_then(|e| e.get("groups")?.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let show = mode == Some("free") || !groups.is_empty() || (mode.is_none() && ext.is_some());
    if !show {
        return None;
    }
    let i = level.map(|l| l.min(4) as usize);
    let per_level = |key: &str, def: &[u64; 5]| -> Option<u64> {
        let i = i?;
        match sect? {
            Some(e) => e
                .get(key)
                .and_then(|a| a.get(i))
                .and_then(Value::as_u64)
                .or(Some(def[i])),
            None => Some(def[i]),
        }
    };
    let scalar = |key: &str, def: u64| -> Option<u64> {
        match sect? {
            Some(e) => e.get(key).and_then(Value::as_u64).or(Some(def)),
            None => Some(def),
        }
    };
    let caps_max = match (
        scalar("bloat_caps", BLOAT_CAPS),
        per_level("bloat_permille", &BLOAT_PERMILLE),
    ) {
        (Some(c), Some(f)) => Some(c * f / 1000),
        _ => None,
    };
    // The newest open group's window: X3's `turns` is the owner turns seen since its last join.
    let newest = groups
        .iter()
        .max_by_key(|g| g.get("seq").and_then(Value::as_u64).unwrap_or(0));
    let window = newest.map(|g| {
        let kernel = g.get("kernel").and_then(Value::as_bool) == Some(true);
        let of = if kernel {
            Some(KERNEL_WIN_TURNS)
        } else {
            scalar("win_turns", WIN_TURNS)
        };
        json!({"kernel": kernel, "of": of, "turns": g.get("turns").and_then(Value::as_u64)})
    });
    let rows: Vec<&Value> = caps
        .and_then(|c| c.get("caps")?.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let state = |r: &Value| {
        r.get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // X6 counts registered, non-core, live caps; seed components, prompt files and caps only seen in
    // use do not count.
    let registered = |r: &&&Value| {
        r.get("registered").and_then(Value::as_bool) == Some(true)
            && !r
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|id| id.starts_with("prompt/"))
    };
    let live = caps.and_then(|c| c.get("caps")?.as_array()).map(|_| {
        rows.iter()
            .filter(registered)
            .filter(|r| LIVE.contains(&state(r).as_str()))
            .count() as u64
    });
    let in_state = |s: &str| -> Vec<Value> {
        let mut v: Vec<String> = rows
            .iter()
            .filter(registered)
            .filter(|r| state(r) == s)
            .filter_map(|r| r.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        v.sort();
        v.into_iter().map(Value::String).collect()
    };
    let quarantine =
        ids(ext.and_then(|e| e.get("quarantine_debt"))).unwrap_or_else(|| in_state("quarantined"));
    let archive =
        ids(ext.and_then(|e| e.get("archive_due"))).unwrap_or_else(|| in_state("archive_due"));
    Some(json!({
        "archive_due": archive,
        "caps": live,
        "caps_max": caps_max,
        "freeze_until": ext.and_then(|e| e.get("freeze_until")).filter(|f| f.is_string()),
        "groups": groups.len(),
        "groups_max": per_level("open_groups", &OPEN_GROUPS),
        "mode": mode,
        "quarantine_debt": quarantine,
        // A pending rollback: its reason, or `true` when it has none.
        "rollback": ext.and_then(|e| e.get("rollback")).filter(|r| r.is_object())
            .map(|r| r.get("reason").filter(|w| w.is_string()).cloned().unwrap_or(Value::Bool(true))),
        "window": window,
    }))
}

/// A copy of `boot.json` for rendering: `env.root` normalized (with `extend`, ADR-011 X11),
/// `selfmap.body_tokens` filled.
pub fn render_boot(raw: &Value, home: &Path) -> Value {
    let mut b = raw.clone();
    let ledger = read_json(&home.join("root/ledger.json"));
    let ceil = read_json(&home.join("root/ceilings.json"));
    let charter = read_json(&home.join("root/charter.json"));
    let ext = read_json(&home.join("root/extend.json"));
    let caps = read_json(&home.join("root/caps.json"));
    let root = raw
        .get("env")
        .and_then(|e| e.get("root"))
        .filter(|r| r.is_object())
        .cloned();
    if (root.is_some() || ledger.is_some() || ceil.is_some() || charter.is_some() || ext.is_some())
        && let Some(env) = b.get_mut("env").and_then(Value::as_object_mut)
    {
        let r = root.unwrap_or(Value::Object(Map::new()));
        let mut r = normalize_root(&r, ledger.as_ref(), ceil.as_ref(), charter.as_ref());
        let mode = r
            .get("ceilings")
            .and_then(|c| c.get("extend")?.get("mode")?.as_str())
            .or_else(|| ceil.as_ref()?.get("extend")?.get("mode")?.as_str())
            .map(str::to_string);
        let level = r.get("level").and_then(Value::as_u64);
        if let (Some(x), Some(m)) = (
            normalize_extend(
                ext.as_ref(),
                caps.as_ref(),
                mode.as_deref(),
                charter.as_ref(),
                level,
            ),
            r.as_object_mut(),
        ) {
            m.insert("extend".into(), x);
        }
        env.insert("root".into(), r);
    }
    if u(raw, &["selfmap", "body_tokens"]).is_none()
        && let Some(t) =
            read_json(&home.join("body/self.json")).and_then(|s| u(&s, &["budget", "tokens"]))
        && let Some(obj) = b.as_object_mut()
    {
        let sm = obj
            .entry("selfmap")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(m) = sm.as_object_mut() {
            m.insert("body_tokens".into(), t.into());
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The kernel's real `env.root` (kernel-supervise boot_file) with the Root's ceilings view.
    #[test]
    fn real_root_shapes_fill_placeholders() {
        let root = json!({
            "audited_good": "3f2a1c0d9e8b3f2a1c0d9e8b3f2a1c0d9e8b3f2a", "level": 0, "max_level": 4, "next": null,
            "ceilings": {"epoch": 3, "level": 0, "v": 1,
                         "money": {"day_limit_musd": 5000000, "day_spent_musd": 10, "total_limit_musd": 25000000,
                                   "total_spent_musd": 7400000},
                         "parallel_bodies": {"active": 1, "limit": 1},
                         "classes": [{"enabled": true, "id": "providers", "min_level": 0},
                                     {"enabled": false, "id": "web", "min_level": 0},
                                     {"enabled": true, "id": "crates", "min_level": 0},
                                     {"enabled": true, "id": "mail", "min_level": 3}]},
            "classes": []
        });
        let ledger = json!({"cap": 0, "level": 0, "max_level": 4, "supported_max_level": 0, "next": null, "last_change": null});
        let r = normalize_root(&root, Some(&ledger), None, None);
        assert_eq!(r["cap"], 0);
        assert_eq!(r["supported_max_level"], 0);
        assert_eq!(r["ceilings"]["day_musd"], 5000000);
        assert_eq!(r["ceilings"]["total_musd"], 25000000);
        assert_eq!(r["ceilings"]["total_used_musd"], 7400000);
        assert_eq!(r["ceilings"]["parallel_bodies"], 1);
        assert_eq!(r["classes"], json!(["crates", "providers"]));
        // Without a ledger view: cap from max and supported when both are known, else absent.
        let r = normalize_root(
            &json!({"level": 0, "max_level": 4, "supported_max_level": 0}),
            None,
            None,
            None,
        );
        assert_eq!(r["cap"], 0);
        let r = normalize_root(&json!({"level": 0, "max_level": 4}), None, None, None);
        assert!(r.get("cap").is_none());
        // The kernel's real env.root has no cap and no supported max: the Root's charter view
        // gives both (root.md §3.12.5), so the cap is min(4, 0).
        let charter =
            json!({"autonomy": {"max_level": 4}, "charter_sha256": "x", "supported_max_level": 0});
        let r = normalize_root(
            &json!({"level": 0, "max_level": 4, "next": null}),
            None,
            None,
            Some(&charter),
        );
        assert_eq!(
            (r["cap"].clone(), r["supported_max_level"].clone()),
            (json!(0), json!(0))
        );
        // §4.5 names already present win.
        let r = normalize_root(
            &json!({"cap": 2, "ceilings": {"day_musd": 1}, "classes": ["models"]}),
            Some(&ledger),
            None,
            None,
        );
        assert_eq!(
            (
                r["cap"].clone(),
                r["ceilings"]["day_musd"].clone(),
                r["classes"].clone()
            ),
            (json!(2), json!(1), json!(["models"]))
        );
    }
}
