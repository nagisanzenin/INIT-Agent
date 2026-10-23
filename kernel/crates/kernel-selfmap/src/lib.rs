#![forbid(unsafe_code)]
//! `kernel-selfmap`: the `self.json` generator (kernel/selfmap.md v0.3).
//!
//! What lives here:
//! - [`body`]: the committed body file (`self_v` 2, committed fields only; v03-decisions D12): the file
//!   inventory and `source_digest` ([`inventory`]), the syn scan ([`rust_scan`]), `@init compensates-for`
//!   markers ([`markers`]), `o200k_base` token counts ([`tokens`]), tools from `init-body --manifest`
//!   ([`manifest`], algorithm `init-tools-v1`), and each extension's WIT inventory ([`wit`]) and
//!   `describe()` result ([`describe`], always in a child process, never in the kernel);
//! - [`drift`]: the G1 drift comparison, the same semantics as `kernel-gate`'s `g1` (selfmap.md §3.8);
//! - [`kernel_tree`]: `kernel-self.json`, the kernel inventory (§3.13);
//! - [`capability`]: the capability self-model `capability-v1` (§3.12);
//! - [`serve`]: the served view `state/kernel/self.json` with its `live` section (§3.11);
//! - [`schema`]: the v2 JSON Schemas (§4.2, §4.2a);
//! - [`cli`]: `init-kernel selfmap …` (§4.1).

pub mod body;
pub mod capability;
pub mod cli;
pub mod describe;
pub mod diag;
pub mod drift;
pub mod icj;
pub mod inventory;
pub mod kernel_tree;
pub mod manifest;
pub mod markers;
pub mod rust_scan;
pub mod schema;
pub mod serve;
pub mod tokens;
pub mod wit;

/// This crate's name, for diagnostics.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

/// `generator.name` (selfmap.md §4.2).
pub const GENERATOR_NAME: &str = "init-kernel selfmap";
/// `generator.version`. Bumped whenever the output changes for the same input (selfmap.md §3.8 item 2).
pub const GENERATOR_VERSION: u64 = 2;
/// `self_v`.
pub const SELF_V: u64 = 2;
/// `generator.tokenizer` (selfmap.md §3.4).
pub const TOKENIZER: &str = "o200k_base";

/// The `generator` object shared by `self.json` and `kernel-self.json`.
pub fn generator_value() -> serde_json::Value {
    serde_json::json!({"name": GENERATOR_NAME, "tokenizer": TOKENIZER, "version": GENERATOR_VERSION})
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, empty directory under `kernel/target/test-homes/WP9/` (BUILD_AGENT_BRIEF: tests never
    /// touch anything outside the repo). Removed with [`remove_home`] by exact path.
    pub fn test_home(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-homes/WP9")
            .join(format!("{name}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("remove stale test home");
        }
        std::fs::create_dir_all(&dir).expect("create test home");
        dir
    }

    /// Removes a directory created by [`test_home`], by its exact path.
    pub fn remove_home(dir: &Path) {
        assert!(dir.to_string_lossy().contains("/target/test-homes/WP9/"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Writes `text` at `root/rel`, creating parents.
    pub fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_and_generator() {
        assert_eq!(super::CRATE, "kernel-selfmap");
        assert_eq!(
            super::generator_value().to_string(),
            r#"{"name":"init-kernel selfmap","tokenizer":"o200k_base","version":2}"#
        );
    }
}
