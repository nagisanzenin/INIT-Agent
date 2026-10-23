//! The profile texts compiled into `init-root` must be byte-identical to the docs' code blocks
//! (kernel/sandbox.md §3.3.1, §3.4, §3.5; root/root.md §3.3.1–§3.3.3 and the §3.3.6 sealed text).
//! The docs are the spec.
#![cfg(target_os = "macos")]

use init_sandbox_profiles::seatbelt::*;

fn scheme_blocks(rel: &str) -> Vec<String> {
    let path = format!("{}/../../docs/{rel}", env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in doc.lines() {
        match (&mut cur, line) {
            (None, "```scheme") => cur = Some(String::new()),
            (Some(_), "```") => out.push(cur.take().unwrap()),
            (Some(b), l) => {
                b.push_str(l);
                b.push('\n');
            }
            (None, _) => {}
        }
    }
    out
}

#[test]
fn sandbox_md_texts_are_verbatim() {
    let b = scheme_blocks("kernel/sandbox.md");
    assert_eq!(b.len(), 3, "kernel/sandbox.md scheme blocks");
    assert_eq!(b[0], format!("{PRELUDE}{BODY_SECTION}"), "§3.3.1 body = PRELUDE + SECTION");
    assert_eq!(b[1], BUILD_SECTION, "§3.4 build");
    assert_eq!(b[2], GIT_READ_SECTION, "§3.5 git-read");
}

#[test]
fn root_md_texts_are_verbatim() {
    let b = scheme_blocks("root/root.md");
    assert_eq!(b.len(), 4, "root/root.md scheme blocks");
    assert_eq!(b[0], ROOT_ALLOWS_TEMPLATE, "§3.3.1 ROOT_ALLOWS");
    assert_eq!(b[1], ZONE_TAIL, "§3.3.2 zone tail");
    assert_eq!(b[2], KERNEL_SECTION, "§3.3.3 kernel SECTION");
    assert_eq!(b[3], SEALED_TAIL, "§3.3.6 sealed text");
    assert_eq!(render_sealed(), format!("{PRELUDE}{}", b[3]), "§3.3.1 profile(kind sealed) = PRELUDE + the sealed text");
}
