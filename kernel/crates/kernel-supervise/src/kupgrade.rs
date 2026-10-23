//! L3 (gated) and L1 (ADR-011 free path): kernel self-upgrade, kernel-side steps (supervise.md §3.15; ADR-011 §1.2).
//! **Stub** in WP4: the request checks are real (they decide whether a `system.swap` may ever be sent, and a refused one
//! demotes); the K0–K3 stages belong to the gate (WP8) and the swap to the Root.
//!
//! X8 (the kernel lane): on `extend.mode = "free"` the level gate is L1, and the Root admits a kernel change only after
//! its conformance probe (root `sup/kprobe.rs`): in a Root-owned probe home the candidate is started with
//! `boot.reason = "kupgrade_smoke"` ([`SMOKE_REASON`]) and must send `hello` (proto 1) within 30 s, become `ready`, ask
//! `ceilings.get`, spawn its LKG body (`$INIT_HOME/bin/init-body`) with `INIT_MODE=check` and see it exit 0, then exit 0
//! within the grace on `stop`, and (second run) answer `prepare_swap` `ok` and exit 75 within 10 s. The probe's boot
//! block and ceilings parse as this crate's types (test below). The isolated smoke path is implemented in `smoke`;
//! production `kstate` export/restore is implemented in `kstate`; free promotion remains a separate K8 requirement.

pub mod candidate;
pub(crate) mod smoke;
pub(crate) mod kstate;

use init_proto::envelope::ErrorBody;
use init_proto::errors::{BusyReason, ErrorCode};
use init_proto::root_link::SwapData;

/// The Root's boot reason for its conformance probe of a kernel candidate (ADR-011 §1.2).
pub const SMOKE_REASON: &str = "kupgrade_smoke";

/// Why `initctl kernel promote` is refused (§3.15 step 2), checked before anything is fetched (the gated path: L3).
pub fn check_request(level: u8, safe: bool, provisional_or_g4: bool, candidate_in_flight: bool) -> Result<(), ErrorBody> {
    check_request_in(false, level, safe, provisional_or_g4, candidate_in_flight)
}

/// [`check_request`] by `extend.mode`: the free path's kernel lane opens at L1 (ADR-011 §1.2, K-C); gated keeps L3. The
/// kernel's check is mandatory: a `system.swap` below the level is a demoting refusal at the Root.
pub fn check_request_in(free: bool, level: u8, safe: bool, provisional_or_g4: bool, candidate_in_flight: bool) -> Result<(), ErrorBody> {
    let need = if free { 1 } else { 3 };
    if level < need {
        return Err(ErrorBody::level(need, level));
    }
    if safe {
        return Err(ErrorBody::new(ErrorCode::SafeMode, "body safe mode: kernel promote is refused"));
    }
    if provisional_or_g4 {
        return Err(ErrorBody::new(ErrorCode::Provisional, "the primary is not at a clean LKG"));
    }
    if candidate_in_flight {
        return Err(ErrorBody::busy(None, Some(BusyReason::KernelCandidate), "another kernel candidate is in flight"));
    }
    Ok(())
}

/// The Root's `swap` progress event: mirrored onto the kernel-upgrade job (WP7/WP8 own jobs).
type SwapHook = std::sync::Arc<dyn Fn(&SwapData) + Send + Sync>;
static SWAP_HOOK: std::sync::Mutex<Option<SwapHook>> = std::sync::Mutex::new(None);
/// Installed by the initctl adapter; the Root event remains the authority for job completion.
pub fn set_swap_hook(hook: SwapHook) {
    *SWAP_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}
pub fn on_swap_event(s: &SwapData) {
    let hook = SWAP_HOOK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(hook) = hook { hook(s); }
    crate::klog::log("kupgrade", &format!("swap {} {}: {}", s.job, s.stage, s.msg));
}

#[cfg(test)]
mod tests {
    use init_proto::root_link::{BootBlock, Ceilings};
    use serde_json::json;

    #[test]
    fn request_rules() {
        assert_eq!(super::check_request(2, false, false, false).unwrap_err().code.as_str(), "E_LEVEL");
        assert_eq!(super::check_request(3, true, false, false).unwrap_err().code.as_str(), "E_SAFE_MODE");
        assert_eq!(super::check_request(3, false, true, false).unwrap_err().code.as_str(), "E_PROVISIONAL");
        assert_eq!(super::check_request(3, false, false, true).unwrap_err().code.as_str(), "E_BUSY");
        assert!(super::check_request(3, false, false, false).is_ok());
    }

    /// X8: the free path's kernel lane opens at L1; L0 is refused with `E_LEVEL{need:1}`.
    #[test]
    fn the_free_path_opens_the_kernel_lane_at_l1() {
        let e = super::check_request_in(true, 0, false, false, false).unwrap_err();
        assert_eq!((e.code.as_str(), e.data.as_ref().map(|d| d["need"].clone())), ("E_LEVEL", Some(json!(1))));
        assert!(super::check_request_in(true, 1, false, false, false).is_ok());
        assert_eq!(super::check_request_in(false, 1, false, false, false).unwrap_err().code.as_str(), "E_LEVEL");
    }

    /// X8: the conformance probe's `hello` result and `ceilings.get` answer (root `sup/kprobe.rs`, byte shapes copied
    /// here) parse as this crate's types, so a conforming kernel can read them.
    #[test]
    fn the_root_probe_boot_block_and_ceilings_parse() {
        let boot = json!({"access": {"access": "none", "full": false, "workspace": null}, "extend": {"mode": "free"},
            "boot": {"cause": null, "kstate": null, "level": 1, "mode": "normal", "panic": null, "reason": super::SMOKE_REASON,
                "tuple": {"audited": null, "current": "probe", "lkg": "probe", "settled": null, "twin": null},
                "tuple_body": {"binary": "b".repeat(64), "ext": {"components": [], "set": null}, "sha": "c".repeat(40)},
                "tuple_kernel": {"built_from": "", "init_kernel": "d".repeat(64), "initctl": "", "source": "agent", "version": "0.1.1"}},
            "charter_sha256": "e".repeat(64), "ladder": {"cap": 1, "evolution_share_max_permille": null, "level": 1},
            "limits": {"hb_interval_s": 5, "max_instances": 4, "parallel_bodies": 1},
            "paths": {"egress_sock": "/r/run/egress.sock", "kernel_git": "/r/lineage/kernel.git"},
            "probe": {"body_mode": "check", "kind": "kupgrade-smoke"}, "root": {"proto": 1, "version": "0.1.0"}});
        let b: BootBlock = serde_json::from_value(boot).expect("the probe's boot block parses");
        assert_eq!(b.boot.reason.as_str(), super::SMOKE_REASON);
        assert_eq!(b.boot.tuple_kernel.source, "agent");
        let c: Ceilings = serde_json::from_value(json!({"extend": {"mode": "free"}, "instances": {"active": 0, "limit": 4}, "level": 1,
            "parallel_bodies": {"active": 0, "limit": 1}, "v": 1})).expect("the probe's ceilings parse");
        assert_eq!(c.parallel_bodies.map(|p| p.limit), Some(1));
    }
}
