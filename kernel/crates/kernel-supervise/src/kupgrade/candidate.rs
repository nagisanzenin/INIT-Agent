//! Kernel candidate preparation, before any build or Root admission.
//!
//! This is deliberately separate from the body G0–G4 pipeline. A prepared source or built
//! artifact is NOT an admitted change: callers still need a sandboxed build, conformance
//! admission through `change.open(scope=k)`, and Root-owned `system.swap`.
use crate::git::{CodedError, Git};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Run only in a Root-created build instance, with cleared environment, isolated Cargo home
/// and `CARGO_TARGET_DIR=Prepared.target`. Never execute this plan in the supervisor itself.
pub const BUILD_ARGS: &[&str] = &[
    "build", "--frozen", "--offline", "--package", "init-kernel", "--bin", "init-kernel",
    "--package", "initctl", "--bin", "initctl", "--profile", "dev",
    "--config", "source.crates-io.replace-with=\"vendored-sources\"",
    "--config", "source.vendored-sources.directory=\"vendor\"",
];

#[derive(Debug)]
pub struct Prepared {
    pub parent_sha: String,
    pub source_sha: String,
    pub source: PathBuf,
    pub target: PathBuf,
}

fn fail(code: &'static str, message: impl Into<String>) -> CodedError {
    CodedError::new(code, message)
}

fn job_id(job: &str) -> bool {
    (1..=64).contains(&job.len()) && job.as_bytes()[0].is_ascii_alphanumeric()
        && job.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Validate an already sandbox-fetched candidate in the dedicated kernel candidate lineage,
/// then export its exact commit into a fresh job directory. Neither moving branch refs nor an
/// agent working directory is used as build input. Existing jobs are never overwritten.
pub fn prepare(git: &Git, parent: &str, candidate: &str, job: &str) -> Result<Prepared, CodedError> {
    if git.git_dir != git.home.join("lineage/kernel-cand.git") {
        return Err(fail("E_ARGS", "kernel candidates require lineage/kernel-cand.git"));
    }
    if !job_id(job) || !crate::git::is_oid(parent) || !crate::git::is_oid(candidate) {
        return Err(fail("E_ARGS", "bounded job id and pinned full commit ids are required"));
    }
    if parent == candidate {
        return Err(fail("E_ARGS", "kernel candidate must change the running commit"));
    }
    git.check_candidate(parent, candidate, "refs/upstream/")?;
    // The caller owns serialization and the kernel-owned cache parent. create_dir is the
    // final atomic reservation: a concurrent/repeated job cannot reuse another job's export.
    let jobs = git.home.join("cache/kernel-candidates");
    std::fs::create_dir_all(&jobs).map_err(|e| fail("E_IO", e.to_string()))?;
    let dir = jobs.join(job);
    std::fs::create_dir(&dir).map_err(|e| fail("E_BUSY", format!("reserve kernel candidate directory: {e}")))?;
    let source = dir.join("src");
    let target = dir.join("out/target");
    git.export_tree(candidate, &source).map_err(|e| fail("E_TREE", e.to_string()))?;
    // A kernel workspace must contain both package manifests. Actual Cargo graph and build
    // script safety are enforced by the subsequent sandboxed build, never inferred here.
    for relative in ["Cargo.toml", "Cargo.lock", "crates/init-kernel/Cargo.toml", "crates/initctl/Cargo.toml"] {
        let path = source.join(relative);
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            return Err(fail("E_TREE", format!("kernel source missing regular {relative}")));
        }
    }
    std::fs::create_dir_all(&target).map_err(|e| fail("E_IO", e.to_string()))?;
    Ok(Prepared { parent_sha: parent.into(), source_sha: candidate.into(), source, target })
}

/// One opened build artifact. The future Root store upload must send THIS fd and digest;
/// reopening the pathname would lose the inode binding. Root still verifies every byte.
#[derive(Debug)]
pub struct Artifact {
    pub file: File,
    pub sha256: String,
    pub bytes: u64,
}

fn artifact(target: &Path, name: &str) -> Result<Artifact, CodedError> {
    // The sandboxed build and all descendants must already be stopped. Refuse directory
    // symlinks as well as the final file; artifact fds remain pinned through Root upload.
    for dir in [target.to_path_buf(), target.join("debug")] {
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir()) {
            return Err(fail("E_BUILD", "kernel artifact directory is missing or symlinked"));
        }
    }
    let path = target.join("debug").join(name);
    let root = target.canonicalize().map_err(|e| fail("E_BUILD", e.to_string()))?;
    let canonical = path.canonicalize().map_err(|e| fail("E_BUILD", e.to_string()))?;
    if !canonical.starts_with(&root) {
        return Err(fail("E_BUILD", "kernel artifact escapes its build target"));
    }
    let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path).map_err(|e| fail("E_BUILD", e.to_string()))?;
    let meta = file.metadata().map_err(|e| fail("E_BUILD", e.to_string()))?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > 512 * 1024 * 1024 || meta.permissions().mode() & 0o111 == 0 {
        return Err(fail("E_BUILD", "kernel artifact must be a nonempty executable regular file <=512MiB"));
    }
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut bytes = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| fail("E_BUILD", e.to_string()))?;
        if n == 0 { break; }
        bytes += n as u64;
        if bytes > 512 * 1024 * 1024 { return Err(fail("E_BUILD", "kernel artifact grew beyond limit")); }
        hash.update(&buf[..n]);
    }
    if bytes != meta.len() { return Err(fail("E_BUILD", "kernel artifact size changed during hashing")); }
    file.seek(SeekFrom::Start(0)).map_err(|e| fail("E_BUILD", e.to_string()))?;
    Ok(Artifact { file, sha256: crate::fsx::hex(&hash.finalize()), bytes })
}

impl Prepared {
    /// Called only after successful sandboxed build exit. No artifact is executed or installed.
    pub fn artifacts(&self) -> Result<(Artifact, Artifact), CodedError> {
        Ok((artifact(&self.target, "init-kernel")?, artifact(&self.target, "initctl")?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testfx::Fx;

    #[test]
    fn kernel_candidate_preparation_pins_source_and_requires_distinct_job() {
        let fx = Fx::new("kernel-prepare");
        for file in ["Cargo.toml", "Cargo.lock", "crates/init-kernel/Cargo.toml", "crates/initctl/Cargo.toml"] {
            fx.write(file, b"# fixture\n");
        }
        let parent = fx.commit("parent");
        fx.write("marker", b"candidate");
        let candidate = fx.commit("candidate");
        let git = Git::with_dir(&fx.git.git, &fx.home, &fx.home.join("lineage/kernel-cand.git"));
        git.init_bare(&git.git_dir).unwrap();
        git.fetch(&fx.agent, "refs/heads/main", "refs/init/candidates/test", "git-upload-pack").unwrap();
        assert!(prepare(&fx.git, &parent, &candidate, "job").is_err());
        assert!(prepare(&git, &parent, &candidate, "../escape").is_err());
        assert!(prepare(&git, &candidate, &parent, "reverse").is_err());
        let prepared = prepare(&git, &parent, &candidate, "job").unwrap();
        assert_eq!(std::fs::read(prepared.source.join("marker")).unwrap(), b"candidate");
        assert_eq!(prepared.source_sha, candidate);
        assert!(prepare(&git, &parent, &candidate, "job").is_err());
        assert!(prepared.artifacts().is_err());
        std::fs::create_dir(prepared.target.join("debug")).unwrap();
        for name in ["init-kernel", "initctl"] {
            let p = prepared.target.join("debug").join(name);
            std::fs::write(&p, b"fixture-binary").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (mut kernel, ctl) = prepared.artifacts().unwrap();
        assert_eq!(kernel.sha256, crate::fsx::sha256_bytes(b"fixture-binary"));
        assert_eq!(ctl.bytes, 14);
        let mut bytes = Vec::new(); kernel.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"fixture-binary");
        let p = prepared.target.join("debug/initctl");
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink("init-kernel", p).unwrap();
        assert!(prepared.artifacts().is_err());
        let fifo = prepared.target.join("debug/initctl");
        std::fs::remove_file(&fifo).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        // No writer or helper process: a blocking read-only open would hang this test.
        let started = std::time::Instant::now();
        assert!(prepared.artifacts().is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        std::fs::remove_dir_all(&fx.dir).unwrap();
    }
}
