//! `kernel propose` (kernel/initctl.md §4.1): read the patch (`O_NOFOLLOW`, regular file,
//! ≤ 1 MiB, UTF-8), build `text` = rationale + patch (≤ 64 KiB), have the backend send
//! `proposal.file{title, text}` to the Root, then keep
//! `state/kernel/proposals/kp-NNNN/{proposal.json, patch.diff}`.

use crate::backend::{Backend, LaneSpec};
use crate::clock;
use crate::fsx;
use crate::paths::{self, Want};
use init_proto::ErrorBody;
use init_proto::errors::ErrorCode;
use init_proto::initctl::{KernelProposeArgs, KernelProposeResult};
use serde_json::json;
use std::path::Path;
use std::sync::Mutex;

/// The patch is at most 1 MiB before encoding.
pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
/// `text` sent to the Root is at most 64 KiB.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The proposals directory.
pub const PROPOSALS_DIR: &str = "state/kernel/proposals";

static NUMBERING: Mutex<()> = Mutex::new(());

/// The next `kp-NNNN` under `dir`.
fn next_id(dir: &Path) -> String {
    let max = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("kp-")).and_then(|d| d.parse::<u32>().ok()))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    format!("kp-{:04}", max + 1)
}

/// Run `kernel propose` for `lane` (serve has checked the lane, state and quota).
pub fn run(home: &Path, lane: &LaneSpec, backend: &dyn Backend, args: &KernelProposeArgs) -> Result<KernelProposeResult, ErrorBody> {
    let path = paths::resolve(lane, "patch", &args.patch, Want::File)?;
    let patch = paths::read_text(&path, "patch", MAX_PATCH_BYTES)?;
    let text = if args.rationale.is_empty() { patch.clone() } else { format!("{}\n\n{}", args.rationale, patch) };
    if text.len() > MAX_TEXT_BYTES {
        return Err(ErrorBody::new(
            ErrorCode::TooLarge,
            format!("rationale + patch is {} bytes; the Root takes at most {MAX_TEXT_BYTES}", text.len()),
        ));
    }
    let root_id = backend.file_proposal(&lane.ctx, &args.title, &text)?;
    let _g = NUMBERING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = home.join(PROPOSALS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("proposals dir: {e}")))?;
    let id = next_id(&dir);
    let pdir = dir.join(&id);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir(&pdir)?;
        fsx::write_atomic(&pdir.join("patch.diff"), patch.as_bytes())?;
        fsx::write_json(
            &pdir.join("proposal.json"),
            &json!({
                "filed_at": clock::now_rfc3339(),
                "id": id,
                "lane": lane.ctx.key(),
                "patch_bytes": patch.len(),
                "rationale": args.rationale,
                "root_id": root_id,
                "status": "filed",
                "title": args.title,
                "v": 1,
            }),
        )
    };
    write().map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("the Root filed {root_id} but the local copy failed: {e}")))?;
    Ok(KernelProposeResult { path: format!("{PROPOSALS_DIR}/{id}/"), proposal: id, root_id, status: "filed".into() })
}
