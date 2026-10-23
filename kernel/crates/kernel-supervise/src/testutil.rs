//! Test homes under `kernel/target/test-homes/wp4/` (BUILD_AGENT_BRIEF safety rules): created
//! per test with a random suffix, removed by exact path.

use std::path::{Path, PathBuf};

/// `kernel/target/test-homes/wp4`.
pub fn test_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp4")
}

/// A fresh, not yet created directory `test-homes/wp4/<name>-<6 hex>` (canonical parent).
pub fn test_dir(name: &str) -> PathBuf {
    let root = test_root();
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    root.join(format!("{name}-{}", crate::sys::random_hex(3)))
}

/// Remove a test directory by its exact path (refuses anything outside the test root).
pub fn remove_test_dir(d: &Path) {
    let root = std::fs::canonicalize(test_root()).unwrap();
    assert!(d.starts_with(&root) && d != root, "refusing to remove {}", d.display());
    let _ = std::fs::remove_dir_all(d);
}
