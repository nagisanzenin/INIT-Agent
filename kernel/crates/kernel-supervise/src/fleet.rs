//! The fleet (supervise.md §3.14). The member runtime (spawn, relay, campaign) is **not built**: it
//! needs `initctl fleet` mechanics and the HGM policy (evolution.md §3.15.5), and ADR-011 leaves the
//! larger sub-agent design to a later phase. What is here: the call sites the supervisor uses (the
//! primary's crash loop and halts stop members, a demotion stops them), and `fleet spawn`'s rules as
//! ADR-011 §1.1 and §4.5 define them, checked locally so no demoting `spawn` reaches the Root:
//! - the level: L2 on the gated path; **any level** on the free path (`extend.mode = "free"`);
//! - `parallel_bodies`: the live bodies (the primary counts) must leave room (`E_CEILING`);
//! - `{agent}`: the sub-agent spec `agents/<agent>/AGENT.md` must exist in the parent version's
//!   T0 tree (`E_NOT_FOUND`); its name is a cap name without `/` (`E_ARGS`);
//! - members never create members (serve refuses `fleet spawn` from a member lane, `E_LANE`), read
//!   the home and never write the workspace (ADR-009; the Root composes their profile).

/// Stop every member (body safe mode, halts, demotion). No members exist yet.
pub fn stop_all(reason: &str) {
    let _ = reason;
}

/// A `level` event lowered the level to `to` (§3.16): below L2 every member stops (gated path;
/// on the free path members are allowed at every level and a demotion does not stop them).
pub fn on_demotion(to: u8) {
    if to < 2 {
        stop_all("demoted");
    }
}

/// `fleet spawn` refusal rules (§3.14, SV-40; ADR-011 §4.5): `E_LEVEL` below L2 on the gated path,
/// and `E_CEILING` when the live bodies (the primary counts) already fill `parallel_bodies`.
pub fn check_spawn(level: u8, live_bodies: u32, parallel_bodies: u32, free: bool) -> Result<(), init_proto::envelope::ErrorBody> {
    if level < 2 && !free {
        return Err(init_proto::envelope::ErrorBody::level(2, level));
    }
    if live_bodies >= parallel_bodies {
        return Err(init_proto::envelope::ErrorBody::ceiling("parallel_bodies", parallel_bodies as u64, live_bodies as u64));
    }
    Ok(())
}

/// The sub-agent spec of `fleet spawn{agent}` (ADR-011 §1.1): `agents/<agent>/AGENT.md` at `sha` in
/// `lineage/body.git`. Returns the spec text (≤ 64 KiB).
pub fn agent_spec(git: &crate::git::Git, sha: &str, agent: &str) -> Result<String, init_proto::envelope::ErrorBody> {
    use init_proto::envelope::ErrorBody;
    use init_proto::errors::ErrorCode;
    if !kernel_gate::evo::grammar::is_cap_name(agent) || agent.contains('/') {
        return Err(ErrorBody::new(ErrorCode::Args, format!("agent `{agent}` must match ^[a-z0-9][a-z0-9._-]{{0,62}}$")));
    }
    let path = format!("agents/{agent}/AGENT.md");
    let oid = git.path_id(sha, &path).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("{path}: {e:?}")))?;
    let oid = oid.ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, format!("no sub-agent spec {path} at {}", &sha[..sha.len().min(12)])))?;
    let b = git.read_blob(&oid).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("{path}: {e:?}")))?;
    if b.len() > 64 * 1024 {
        return Err(ErrorBody::new(ErrorCode::TooLarge, format!("{path} is over 64 KiB")));
    }
    String::from_utf8(b).map_err(|_| ErrorBody::new(ErrorCode::Args, format!("{path} is not UTF-8")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn sv40_local_refusals() {
        assert_eq!(super::check_spawn(1, 1, 4, false).unwrap_err().code.as_str(), "E_LEVEL");
        assert_eq!(super::check_spawn(2, 2, 2, false).unwrap_err().code.as_str(), "E_CEILING");
        assert!(super::check_spawn(2, 1, 2, false).is_ok());
    }

    /// ADR-011 §4.5 (X2): on the free path members are allowed from L0, still bounded by
    /// `parallel_bodies`; `{agent}` names a spec in the parent's T0 tree.
    #[test]
    fn x2_fleet_spawn_from_l0_with_an_agent_spec() {
        assert!(super::check_spawn(0, 1, 4, true).is_ok());
        assert_eq!(super::check_spawn(0, 4, 4, true).unwrap_err().code.as_str(), "E_CEILING");
        let fx = crate::git::testfx::Fx::new("fleet-agent");
        fx.write("agents/reviewer/AGENT.md", b"role: review diffs\n");
        let sha = fx.commit("agent spec");
        fx.sync();
        assert_eq!(super::agent_spec(&fx.git, &sha, "reviewer").unwrap(), "role: review diffs\n");
        assert_eq!(super::agent_spec(&fx.git, &sha, "planner").unwrap_err().code.as_str(), "E_NOT_FOUND");
        assert_eq!(super::agent_spec(&fx.git, &sha, "../x").unwrap_err().code.as_str(), "E_ARGS");
        assert_eq!(super::agent_spec(&fx.git, &sha, "a/b").unwrap_err().code.as_str(), "E_ARGS");
    }
}
