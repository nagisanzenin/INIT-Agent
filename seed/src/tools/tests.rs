//! Tool tests (tools.md §8 items 1–7). Every file lives in a temp home under
//! `seed/target/test-homes/wp11/`; every process signalled is one these tests spawned.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::bash::{CaptureSpec, Captured, RunSpec, group_alive, run_group};
use super::ext::OutcomeKind;
use super::testutil::FakeHost;
use super::testutil::{Bench, text};
use super::*;
use crate::model::ContentBlock;

fn write(b: &Bench, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = b.work().join(name);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&p, bytes).unwrap();
    p
}

fn first_line(r: &Ran) -> String {
    text(r).lines().next().unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------------------------
// 7. Registry
// ---------------------------------------------------------------------------------------------

#[test]
fn registry_bytes_are_the_doc_block() {
    assert_eq!(REGISTRY_JSON.len(), 3_405);
    // specs() re-serialized with sorted keys gives the same bytes.
    let v: Vec<Value> = specs().iter().map(|s| s.to_value()).collect();
    let bytes = crate::log::icj::to_vec(&Value::Array(v)).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), REGISTRY_JSON);
    let names: Vec<String> = specs().into_iter().map(|s| s.name).collect();
    assert_eq!(names, NAMES);
    // WP12's head golden (inside the crate) holds the same five objects.
    let wp12: Value = serde_json::from_str(include_str!("../context/testdata/tools.json")).unwrap();
    assert_eq!(
        crate::log::icj::to_vec(&wp12).unwrap(),
        REGISTRY_JSON.as_bytes()
    );
    // The doc golden: tools.md §4.1's block, minified with sorted keys. The docs live in the
    // repository, not in the body: a body checkout (the gate's G0 runs this test on one) has no
    // `../docs`, and then only the in-crate golden above applies.
    let doc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/body/tools.md");
    let Ok(doc) = std::fs::read_to_string(&doc_path) else {
        eprintln!(
            "NOTE: {} not found; doc golden skipped (body checkout)",
            doc_path.display()
        );
        return;
    };
    let i = doc.find("### 4.1 The registry").unwrap();
    let blk = &doc[i..];
    let s = blk.find("```json\n").unwrap() + 8;
    let e = s + blk[s..].find("\n```").unwrap();
    let parsed: Value = serde_json::from_str(&blk[s..e]).unwrap();
    assert_eq!(
        crate::log::icj::to_vec(&parsed).unwrap(),
        REGISTRY_JSON.as_bytes()
    );
    assert_eq!(wp12, parsed);
}

// ---------------------------------------------------------------------------------------------
// Validation (loop.md §3.6, tools.md §4.6)
// ---------------------------------------------------------------------------------------------

#[test]
fn validation_errors_teach() {
    let b = Bench::new("validate");
    let r = b.run("grep", json!({"pattern": "x"}));
    assert_eq!(
        text(&r),
        "error: unknown_tool: no tool named \"grep\". Available tools: bash, edit, ext, read, write."
    );
    let mut c = b.call("edit", json!({}));
    c.raw_arguments = Some("{\"path\": \"a".into());
    let r = b.run_call(&c);
    assert!(r.result.is_error);
    assert!(
        text(&r)
            .starts_with("error: invalid_json: the arguments for \"edit\" are not valid JSON ("),
        "{}",
        text(&r)
    );
    assert!(text(&r).ends_with("Re-issue the call with valid JSON."));
    let r = b.run("read", json!({"file_path": "x"}));
    assert_eq!(
        text(&r),
        "error: invalid_input: unknown field \"file_path\"\nhint: did you mean \"path\"?"
    );
    let r = b.run(
        "edit",
        json!({"path": "x", "edits": [{"old_string": "a", "new_text": "b"}]}),
    );
    assert_eq!(
        text(&r),
        "error: invalid_input: edits[0]: unknown field \"old_string\"\nhint: did you mean \"old_text\"?"
    );
    let r = b.run(
        "edit",
        json!({"path": "x", "edits": [{"old_text": 1, "new_text": "b"}]}),
    );
    assert_eq!(
        first_line(&r),
        "error: invalid_input: edits[0].old_text: expected a string, found a number"
    );
    let r = b.run("read", json!({}));
    assert_eq!(
        first_line(&r),
        "error: invalid_input: path: missing required field"
    );
    let r = b.run("read", json!({"path": ""}));
    assert_eq!(
        first_line(&r),
        "error: invalid_input: path: must not be empty"
    );
}

#[test]
fn path_resolution_is_lexical() {
    let cwd = std::path::Path::new("/w/work");
    let cx = PathCx {
        cwd,
        home: Some(std::path::Path::new("/h")),
        init_home: Some(std::path::Path::new("/i")),
    };
    let r = |p: &str| resolve(p, &cx).unwrap().display().to_string();
    assert_eq!(r("a/./b//c"), "/w/work/a/b/c");
    assert_eq!(r("../x"), "/w/x");
    assert_eq!(r("~/f"), "/h/f");
    assert_eq!(r("~"), "/h");
    assert_eq!(
        r("$INIT_HOME/state/ext/catalog.json"),
        "/i/state/ext/catalog.json"
    );
    assert_eq!(r("/a/b/../../.."), "/");
    assert_eq!(r("$HOME/x"), "/w/work/$HOME/x");
}

// ---------------------------------------------------------------------------------------------
// 1. read
// ---------------------------------------------------------------------------------------------

#[test]
fn read_pages_through_a_long_file() {
    let b = Bench::new("read-page");
    let body: String = (1..=5000).map(|i| format!("l{i}\n")).collect();
    write(&b, "f.txt", body.as_bytes());
    let r = b.run("read", json!({"path": "f.txt"}));
    let t = text(&r);
    assert!(!r.result.is_error);
    assert!(t.starts_with("     1\tl1\n     2\tl2\n"));
    assert!(t.contains("  2000\tl2000\n"));
    assert!(!t.contains("l2001"));
    assert!(t.ends_with("\n[Showing lines 1-2000 of 5000. Use offset=2001 to continue.]"));
    assert_eq!(r.summary, "2000 lines");
    let t = text(&b.run("read", json!({"path": "f.txt", "offset": 2001})));
    assert!(t.starts_with("  2001\tl2001\n"));
    assert!(t.ends_with("[Showing lines 2001-4000 of 5000. Use offset=4001 to continue.]"));
    let t = text(&b.run(
        "read",
        json!({"path": "f.txt", "offset": 4001, "limit": 5000}),
    ));
    assert!(t.starts_with("  4001\tl4001\n"));
    assert!(t.ends_with("  5000\tl5000\n"));
    assert!(!t.contains("Showing"));
    let t = text(&b.run("read", json!({"path": "f.txt", "offset": 10, "limit": 2})));
    assert_eq!(
        t,
        "    10\tl10\n    11\tl11\n\n[Showing lines 10-11 of 5000. Use offset=12 to continue.]"
    );
}

#[test]
fn read_byte_cap_long_lines_crlf_utf8_empty_offset() {
    let b = Bench::new("read-edge");
    let line = "x".repeat(100);
    let body: String = (0..1000).map(|_| format!("{line}\n")).collect();
    write(&b, "wide.txt", body.as_bytes());
    let t = text(&b.run("read", json!({"path": "wide.txt"})));
    let shown = t.lines().filter(|l| l.contains('\t')).count();
    assert!(shown < 1000 && shown > 400, "{shown}");
    let (lines, _) = t.split_once("\n\n[Showing").unwrap();
    assert!(lines.len() < 51_200, "{} bytes, {shown} lines", lines.len());
    assert_eq!(shown, 474);
    assert!(t.ends_with(&format!(
        "[Showing lines 1-{shown} of 1000. Use offset={} to continue.]",
        shown + 1
    )));
    // A 10,000-byte line is cut at 2,000 bytes with a hint.
    let long = format!("{}\nshort\n", "é".repeat(5_000));
    write(&b, "long.txt", long.as_bytes());
    let t = text(&b.run("read", json!({"path": "long.txt"})));
    let first = t.lines().next().unwrap();
    assert!(first.contains("… [line truncated: 10,000 bytes; see it with bash: sed -n '1p' long.txt | cut -c 2001-4000]"));
    assert_eq!(
        first
            .split('\t')
            .nth(1)
            .unwrap()
            .split('…')
            .next()
            .unwrap()
            .len(),
        2_000
    );
    assert!(t.contains("     2\tshort\n"));
    // CRLF.
    write(&b, "crlf.txt", b"a\r\nb\r\n");
    assert_eq!(
        text(&b.run("read", json!({"path": "crlf.txt"}))),
        "     1\ta\n     2\tb\n"
    );
    // Invalid UTF-8.
    write(&b, "bad.txt", b"ok\n\xff\xfe x\n");
    let t = text(&b.run("read", json!({"path": "bad.txt"})));
    assert!(t.contains("     2\t\u{FFFD}\u{FFFD} x\n"));
    assert!(t.ends_with("[file is not valid UTF-8; invalid bytes shown as U+FFFD]\n"));
    // Empty file; offset past the end; last line without newline.
    write(&b, "empty.txt", b"");
    assert_eq!(
        text(&b.run("read", json!({"path": "empty.txt"}))),
        "[empty file]"
    );
    write(&b, "three.txt", b"1\n2\n3");
    let r = b.run("read", json!({"path": "three.txt", "offset": 500}));
    assert_eq!(
        text(&r),
        "error: invalid_input: offset 500 is past the end of the file (3 lines)"
    );
    assert_eq!(
        text(&b.run("read", json!({"path": "three.txt", "offset": 3}))),
        "     3\t3\n"
    );
    let t = text(&b.run("read", json!({"path": "three.txt", "limit": 1})));
    assert!(t.ends_with("[Showing lines 1-1 of 3. Use offset=2 to continue.]"));
}

#[test]
fn read_errors_directory_missing_binary() {
    let b = Bench::new("read-err");
    std::fs::create_dir_all(b.work().join("src/demo")).unwrap();
    write(&b, "src/loop.rs", b"x");
    write(&b, "src/lock.rs", b"x");
    let r = b.run("read", json!({"path": "src/demo"}));
    assert_eq!(
        text(&r),
        "error: is_directory: src/demo is a directory\nhint: use bash: ls -la src/demo"
    );
    let r = b.run("read", json!({"path": "src/lop.rs"}));
    assert_eq!(
        text(&r),
        "error: not_found: src/lop.rs does not exist\nhint: parent exists; similar: src/loop.rs, src/lock.rs"
    );
    let r = b.run("read", json!({"path": "nope/x.rs"}));
    assert!(text(&r).contains("hint: the parent directory nope does not exist either"));
    write(&b, "bin", b"abc\0def");
    let r = b.run("read", json!({"path": "bin"}));
    assert_eq!(
        text(&r),
        "error: binary_file: bin is binary (contains NUL bytes)\nhint: inspect with bash: file bin; xxd bin | head -50"
    );
}

#[test]
fn read_images() {
    let mut b = Bench::new("read-img");
    let png = [
        &[0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A][..],
        b"rest\0",
    ]
    .concat();
    write(&b, "a.png", &png);
    write(&b, "a.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]);
    write(&b, "a.gif", b"GIF89a\0\0");
    write(&b, "a.webp", b"RIFF\0\0\0\0WEBPVP8 ");
    for (f, mt) in [
        ("a.png", "image/png"),
        ("a.jpg", "image/jpeg"),
        ("a.gif", "image/gif"),
        ("a.webp", "image/webp"),
    ] {
        let r = b.run("read", json!({"path": f}));
        assert!(!r.result.is_error, "{f}");
        assert_eq!(r.result.content.len(), 2);
        assert!(
            matches!(&r.result.content[1], ContentBlock::Image { media_type, .. } if media_type == mt)
        );
    }
    let r = b.run("read", json!({"path": "a.png"}));
    assert_eq!(text(&r), "[image: image/png, 13 bytes]");
    let ContentBlock::Image { data_b64, .. } = &r.result.content[1] else {
        panic!()
    };
    assert_eq!(crate::r#loop::hooks::base64_decode(data_b64).unwrap(), png);
    let mut big = png.clone();
    big.resize(3_750_001, 0);
    write(&b, "big.png", &big);
    let r = b.run("read", json!({"path": "big.png"}));
    assert!(first_line(&r).starts_with("error: too_large: image is 3,750,001 bytes"));
    assert!(text(&r).contains("hint: downscale with bash, e.g. sips -Z 1568"));
    b.images = false;
    let r = b.run("read", json!({"path": "a.png"}));
    assert!(!r.result.is_error);
    assert_eq!(
        text(&r),
        "[image file: image/png, 13 bytes; the current model (mock-frontier-1) has no image input]"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. write
// ---------------------------------------------------------------------------------------------

#[test]
fn write_creates_replaces_and_keeps_links_and_modes() {
    let b = Bench::new("write");
    let r = b.run(
        "write",
        json!({"path": "a/b/c.txt", "content": "one\ntwo\n"}),
    );
    assert_eq!(text(&r), "Wrote 8 bytes (2 lines) to a/b/c.txt (new file).");
    assert_eq!(
        std::fs::read_to_string(b.work().join("a/b/c.txt")).unwrap(),
        "one\ntwo\n"
    );
    let p = write(&b, "run.sh", b"#!/bin/sh\n");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = b.run(
        "write",
        json!({"path": "run.sh", "content": "#!/bin/sh\necho hi"}),
    );
    assert_eq!(
        text(&r),
        "Wrote 17 bytes (2 lines) to run.sh (replaced 10 bytes)."
    );
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755
    );
    // Through a symlink: the link stays a link.
    write(&b, "real.txt", b"old");
    std::os::unix::fs::symlink("real.txt", b.work().join("link.txt")).unwrap();
    b.run("write", json!({"path": "link.txt", "content": "new"}));
    assert!(
        std::fs::symlink_metadata(b.work().join("link.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(b.work().join("real.txt")).unwrap(),
        "new"
    );
    // A dangling link is written through too.
    std::os::unix::fs::symlink("made.txt", b.work().join("dangling")).unwrap();
    b.run("write", json!({"path": "dangling", "content": "m"}));
    assert_eq!(
        std::fs::read_to_string(b.work().join("made.txt")).unwrap(),
        "m"
    );
    std::fs::create_dir_all(b.work().join("d")).unwrap();
    let r = b.run("write", json!({"path": "d", "content": "x"}));
    assert_eq!(first_line(&r), "error: is_directory: d is a directory");
    let big = "x".repeat((4 << 20) + 1);
    let r = b.run("write", json!({"path": "big", "content": big}));
    assert_eq!(
        first_line(&r),
        "error: too_large: content is 4,194,305 bytes; write takes at most 4 MiB"
    );
}

#[test]
fn write_never_leaves_a_temp_file() {
    let b = Bench::new("write-tmp");
    write(&b, "keep.txt", b"original");
    super::write::FAIL_RENAME.with(|c| c.set(true));
    let r = b.run("write", json!({"path": "keep.txt", "content": "new"}));
    assert!(r.result.is_error);
    assert_eq!(
        std::fs::read_to_string(b.work().join("keep.txt")).unwrap(),
        "original"
    );
    let names: Vec<String> = std::fs::read_dir(b.work())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["keep.txt"]);
    // A read-only directory: permission_denied, nothing left behind.
    let ro = b.work().join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = b.run("write", json!({"path": "ro/x.txt", "content": "x"}));
    assert!(first_line(&r).starts_with("error: permission_denied: ro/x.txt: permission denied"));
    assert!(text(&r).contains("hint: writable roots are body/"));
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 0);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn concurrent_readers_never_see_a_partial_file() {
    let b = Bench::new("write-race");
    let a = "A".repeat(200_000);
    let z = "Z".repeat(150_000);
    write(&b, "f", a.as_bytes());
    let stop = AtomicBool::new(false);
    let path = b.work().join("f");
    std::thread::scope(|s| {
        let h = s.spawn(|| {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                let got = std::fs::read(&path).unwrap();
                assert!(
                    got == a.as_bytes() || got == z.as_bytes(),
                    "partial read of {} bytes",
                    got.len()
                );
                n += 1;
            }
            n
        });
        for i in 0..1000 {
            let c = if i % 2 == 0 { &z } else { &a };
            assert!(
                !b.run("write", json!({"path": "f", "content": c}))
                    .result
                    .is_error
            );
        }
        stop.store(true, Ordering::Relaxed);
        assert!(h.join().unwrap() > 0);
    });
}

// ---------------------------------------------------------------------------------------------
// 3. edit
// ---------------------------------------------------------------------------------------------

fn edit(b: &Bench, path: &str, edits: &[(&str, &str)]) -> Ran {
    let e: Vec<Value> = edits
        .iter()
        .map(|(o, n)| json!({"old_text": o, "new_text": n}))
        .collect();
    b.run("edit", json!({"path": path, "edits": e}))
}

fn read_back(b: &Bench, p: &str) -> String {
    std::fs::read_to_string(b.work().join(p)).unwrap()
}

#[test]
fn edit_tier1_unique_and_ambiguous() {
    let b = Bench::new("edit-t1");
    write(&b, "f", b"alpha\nbeta\ngamma\n");
    let r = edit(&b, "f", &[("beta", "BETA")]);
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(read_back(&b, "f"), "alpha\nBETA\ngamma\n");
    assert_eq!(r.summary, "1 replacement");
    assert!(text(&r).starts_with(
        "Edited f: 1 replacement.\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma"
    ));
    write(&b, "g", b"aaa\n");
    let r = edit(&b, "g", &[("aa", "b")]);
    assert_eq!(
        text(&r),
        "error: ambiguous_match: edits[0].old_text matches 2 places in g\nhint: found 2 matches (lines 1, 1); add surrounding lines to old_text so it matches one place."
    );
    write(&b, "h", b"x = 1\ny = 2\nx = 1\n");
    let r = edit(&b, "h", &[("x = 1", "x = 3")]);
    assert!(text(&r).contains("found 2 matches (lines 1, 3)"));
}

#[test]
fn edit_tier2_trailing_space_and_curly_quotes() {
    let b = Bench::new("edit-t2");
    write(
        &b,
        "f",
        "say(\u{201C}hi\u{201D})   \nnext\u{00A0}line\nend\n".as_bytes(),
    );
    let r = edit(&b, "f", &[("say(\"hi\")\nnext line\n", "say(\"bye\")\n")]);
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(read_back(&b, "f"), "say(\"bye\")\nend\n");
    assert!(text(&r).starts_with(
        "Edited f: 1 replacement; edits[0] matched ignoring trailing whitespace and punctuation variants."
    ));
}

#[test]
fn edit_tier3_indentation() {
    let b = Bench::new("edit-t3");
    let src = "def f():\n    if x:\n        return 1\n    return 2\n";
    write(&b, "a.py", src.as_bytes());
    // The model's indent is short by 4 spaces: re-indented.
    let r = edit(
        &b,
        "a.py",
        &[(
            "if x:\n    return 1\n",
            "if x:\n    return 10\nelse:\n    pass\n",
        )],
    );
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(
        read_back(&b, "a.py"),
        "def f():\n    if x:\n        return 10\n    else:\n        pass\n    return 2\n"
    );
    assert!(text(&r).contains("edits[0] matched ignoring indentation (re-indented by 4 spaces)"));
    // Long by a tab: stripped.
    write(&b, "b.go", b"func f() {\n\treturn 1\n}\n");
    let r = edit(&b, "b.go", &[("\t\treturn 1\n", "\t\treturn 2\n")]);
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(read_back(&b, "b.go"), "func f() {\n\treturn 2\n}\n");
    assert!(text(&r).contains("(indentation reduced by 1 tab)"));
    // Inconsistent deltas: no match.
    write(&b, "c.py", b"a:\n    b\n        c\n");
    let r = edit(&b, "c.py", &[("a:\n  b\n    c\n", "z")]);
    assert!(
        first_line(&r).starts_with("error: no_match:"),
        "{}",
        text(&r)
    );
    // The no_match hint points at the first line.
    assert!(text(&r).contains(
        "hint: the first line of old_text appears (ignoring indentation) at line 1; the lines after it differ."
    ));
    let r = edit(&b, "c.py", &[("zzz", "y")]);
    assert!(text(&r).ends_with("hint: no line of the file matches the first line of old_text; the file may have changed since you read it. Re-read it."));
}

#[test]
fn edit_crlf_bom_overlap_nochange_limits() {
    let b = Bench::new("edit-misc");
    write(&b, "w.txt", "\u{FEFF}one\r\ntwo\r\nthree\r\n".as_bytes());
    let r = edit(&b, "w.txt", &[("two\r\n", "2\r\nTWO\r\n")]);
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(
        read_back(&b, "w.txt"),
        "\u{FEFF}one\r\n2\r\nTWO\r\nthree\r\n"
    );
    write(&b, "o", b"a\nb\nc\nd\n");
    let r = edit(&b, "o", &[("c\n", "C\n"), ("b\nc", "x")]);
    assert_eq!(
        text(&r),
        "error: overlapping_edits: edits[0] and edits[1] overlap\nhint: edits[0] (lines 3-3) and edits[1] (lines 2-3) overlap; merge them into one edit."
    );
    let r = edit(&b, "o", &[("b", "x"), ("b", "y")]);
    assert!(first_line(&r).starts_with("error: overlapping_edits: edits[0] and edits[1] overlap"));
    let r = edit(&b, "o", &[("b", "b")]);
    assert_eq!(text(&r), "error: no_change: the edits leave o unchanged");
    let r = edit(&b, "o", &[("", "b")]);
    assert_eq!(text(&r), "error: invalid_input: edits[0].old_text is empty");
    // 64 edits are fine, 65 are not.
    let body: String = (0..65).map(|i| format!("line{i:03}\n")).collect();
    write(&b, "many", body.as_bytes());
    let olds: Vec<String> = (0..64).map(|i| format!("line{i:03}")).collect();
    let pairs: Vec<(&str, &str)> = olds.iter().map(|o| (o.as_str(), "X")).collect();
    let r = edit(&b, "many", &pairs);
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(r.summary, "64 replacements");
    let olds: Vec<String> = (0..65).map(|i| format!("q{i}")).collect();
    let pairs: Vec<(&str, &str)> = olds.iter().map(|o| (o.as_str(), "X")).collect();
    let r = edit(&b, "many", &pairs);
    assert_eq!(
        text(&r),
        "error: invalid_input: edits: at most 64 edits per call (got 65); split them over several calls"
    );
    std::fs::create_dir_all(b.work().join("dir")).unwrap();
    assert_eq!(
        first_line(&edit(&b, "dir", &[("a", "b")])),
        "error: is_directory: dir is a directory"
    );
    assert_eq!(
        first_line(&edit(&b, "missing", &[("a", "b")])),
        "error: not_found: missing does not exist"
    );
    write(&b, "bin", b"\xff\xfe");
    assert_eq!(
        first_line(&edit(&b, "bin", &[("a", "b")])),
        "error: binary_file: bin is not valid UTF-8; edit needs a UTF-8 text file"
    );
}

#[test]
fn edit_diff_golden_with_line_shifts() {
    let b = Bench::new("edit-diff");
    let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    write(&b, "src/f.rs", body.as_bytes());
    let r = edit(
        &b,
        "src/f.rs",
        &[
            ("line 5\n", "line 5a\nline 5b\nline 5c\n"),
            ("line 20\n", ""),
            ("line 7\n", "line seven\n"),
        ],
    );
    let want = "Edited src/f.rs: 3 replacements.
--- a/src/f.rs
+++ b/src/f.rs
@@ -3,7 +3,9 @@
 line 3
 line 4
-line 5
+line 5a
+line 5b
+line 5c
 line 6
-line 7
+line seven
 line 8
 line 9
@@ -18,5 +20,4 @@
 line 18
 line 19
-line 20
 line 21
 line 22";
    assert_eq!(text(&r), want);
}

#[test]
fn edit_diff_is_capped() {
    let b = Bench::new("edit-cap");
    let body: String = (1..=300).map(|i| format!("{i}\n")).collect();
    write(&b, "f", body.as_bytes());
    let newb: String = (1..=300).map(|i| format!("n{i}\n")).collect();
    let r = edit(&b, "f", &[(&body, &newb)]);
    let t = text(&r);
    assert!(
        t.ends_with("[diff truncated: 481 more lines; read the file to check the result]"),
        "{}",
        &t[t.len() - 200..]
    );
    assert_eq!(t.lines().count(), 1 + 2 + 120 + 1);
}

/// Edit, then the reverse edit, restores the original bytes (random files, unique substrings).
#[test]
fn edit_property_reverse_restores() {
    let b = Bench::new("edit-prop");
    let mut rng = crate::log::testutil::Rng(0x9e37_79b9_7f4a_7c15);
    let mut done = 0;
    for case in 0..300 {
        let n = 1 + rng.below(40) as usize;
        let mut s = String::new();
        for _ in 0..n {
            let words = 1 + rng.below(6);
            for _ in 0..words {
                s.push_str(
                    ["ab", "c", "  ", "\t", "é", "x1", "{", "}", "a"][rng.below(9) as usize],
                );
            }
            s.push('\n');
        }
        let bytes = s.as_bytes();
        let start = rng.below(s.len() as u64) as usize;
        let len = 1 + rng.below(20) as usize;
        let (mut st, mut en) = (start, (start + len).min(s.len()));
        while !s.is_char_boundary(st) {
            st -= 1;
        }
        while !s.is_char_boundary(en) {
            en += 1;
        }
        let old = &s[st..en];
        if old.is_empty() || s.matches(old).count() != 1 || s.find(old) != s.rfind(old) {
            continue;
        }
        // Overlapping occurrences count too: require exactly one start position.
        let starts = (0..s.len())
            .filter(|&i| s.is_char_boundary(i) && s[i..].starts_with(old))
            .count();
        if starts != 1 {
            continue;
        }
        let new = format!("<<{case}>>");
        let f = format!("p{case}");
        write(&b, &f, bytes);
        let r = edit(&b, &f, &[(old, &new)]);
        assert!(!r.result.is_error, "case {case}: {}", text(&r));
        let r = edit(&b, &f, &[(&new, old)]);
        assert!(!r.result.is_error, "case {case}: {}", text(&r));
        assert_eq!(read_back(&b, &f).as_bytes(), bytes, "case {case}");
        done += 1;
    }
    assert!(done > 100, "{done}");
}

// ---------------------------------------------------------------------------------------------
// 4. bash
// ---------------------------------------------------------------------------------------------

fn bash(b: &Bench, cmd: &str) -> Ran {
    b.run("bash", json!({"command": cmd}))
}

#[test]
fn bash_basics() {
    let b = Bench::new("bash-basic");
    let r = bash(&b, "echo hi");
    let t = text(&r);
    assert!(t.starts_with("hi\n[exit_code=0 wall_time="), "{t}");
    assert!(!r.result.is_error);
    assert_eq!(r.result.meta.exit_code, Some(0));
    assert!(r.summary.starts_with("exit 0, 1 lines"));
    let r = bash(&b, "exit 3");
    assert!(r.result.is_error);
    assert_eq!(r.result.meta.exit_code, Some(3));
    assert!(text(&r).starts_with("[exit_code=3 wall_time="));
    let r = bash(&b, "echo out1; echo err1 >&2; echo out2");
    assert!(text(&r).starts_with("out1\nerr1\nout2\n"));
    let r = bash(&b, "yes | head -c 10");
    assert!(text(&r).starts_with("y\ny\ny\ny\ny\n[exit_code=0"));
    let r = bash(&b, "kill -9 $$");
    assert!(
        text(&r).starts_with("[exit_code=none signal=9 wall_time="),
        "{}",
        text(&r)
    );
    assert_eq!(r.result.meta.signal, Some(9));
    // cd does not persist; the cwd is the body's.
    bash(&b, "cd /tmp");
    let r = bash(&b, "pwd");
    assert!(text(&r).starts_with(&format!("{}\n", b.work().display())));
}

#[test]
fn bash_env() {
    let rc = super::testutil::homes_root().join(format!("bash-env-rc-{}", std::process::id()));
    std::fs::create_dir_all(&rc).unwrap();
    let rcfile = rc.join("rc.sh");
    std::fs::write(&rcfile, "echo SOURCED\n").unwrap();
    let b = Bench::with(
        "bash-env",
        None,
        &[
            ("CARGO_TARGET_DIR", Some("/already/set")),
            ("BASH_ENV", Some(rcfile.to_str().unwrap())),
            ("NO_COLOR", None),
        ],
    );
    let c = b.call("bash", json!({"command": "echo $INIT_CALL_ID $NO_COLOR $GIT_EDITOR $PAGER $CARGO_TARGET_DIR $INIT_MODEL"}));
    let r = b.run_call(&c);
    let t = text(&r);
    assert!(!t.contains("SOURCED"), "{t}");
    assert!(
        t.starts_with(&format!(
            "{} 1 true cat /already/set mock-frontier-1\n",
            c.id
        )),
        "{t}"
    );
    let r = bash(&b, "echo $INIT_SPILL_DIR $TERM $CARGO_TERM_COLOR");
    assert!(text(&r).starts_with(&format!("{} dumb never\n", b.spill.display())));
    let b2 = Bench::new("bash-env2");
    let r = bash(&b2, "echo $CARGO_TARGET_DIR");
    assert!(text(&r).starts_with(&format!("{}/work/target\n", b2.home.path.display())));
    let _ = std::fs::remove_dir_all(&rc);
}

#[test]
fn bash_head_tail_spill_ansi_cr() {
    let b = Bench::new("bash-spill");
    let r = bash(&b, "seq 1 200000");
    let t = text(&r);
    assert!(!r.result.is_error);
    assert!(t.starts_with("1\n2\n3\n"));
    assert!(t.contains("\n[... "));
    assert!(
        t.contains(" lines (") && t.contains("bytes) omitted; full output (1,288,895 bytes) in "),
        "{}",
        &t[3990..4300]
    );
    assert!(t.contains("199999\n200000\n[exit_code=0"));
    let spill = r.result.meta.spill_path.clone().unwrap();
    assert!(spill.ends_with(&format!("{}.txt", super::bash::sanitize(&r.result.call_id))));
    let full = std::fs::read(&spill).unwrap();
    let want: String = (1..=200_000).map(|i| format!("{i}\n")).collect();
    assert_eq!(full, want.as_bytes());
    assert!(t.len() < 31_000);
    // ANSI stripped inline, raw in the spill file.
    let r = bash(
        &b,
        "printf '\\033[31mred\\033[0m \\033]0;title\\007ok\\n'; seq 1 10000 | sed 's/^/\\x1b[1m/'",
    );
    let t = text(&r);
    assert!(t.starts_with("red ok\n1\n2\n"), "{:?}", &t[..40]);
    assert!(!t.contains('\u{1b}'));
    let raw = std::fs::read(r.result.meta.spill_path.unwrap()).unwrap();
    assert!(raw.windows(5).any(|w| w == b"\x1b[31m"));
    // \r progress bars collapse.
    let r = bash(&b, "printf '10%%\\r50%%\\r100%%\\ndone\\n'");
    assert!(text(&r).starts_with("100%\ndone\n"), "{}", text(&r));
    // Spill names collide → -2.
    let c = b.call("bash", json!({"command": "seq 1 20000"}));
    let r1 = b.run_call(&c);
    let r2 = b.run_call(&c);
    assert!(r2.result.meta.spill_path.unwrap().ends_with("-2.txt"));
    assert!(r1.result.meta.spill_path.is_some());
}

#[test]
fn bash_timeout_term_then_kill() {
    let b = Bench::new("bash-timeout");
    let t0 = Instant::now();
    let r = b.run("bash", json!({"command": "sleep 5", "timeout": 1}));
    let wall = t0.elapsed();
    assert!(wall < Duration::from_millis(1_500), "{wall:?}");
    assert!(r.result.is_error);
    assert!(
        text(&r).starts_with("[timeout after 1s: process group killed (TERM) wall_time="),
        "{}",
        text(&r)
    );
    assert_eq!(r.summary, "timeout");
    let t0 = Instant::now();
    let r = b.run(
        "bash",
        json!({"command": "trap '' TERM; sleep 30", "timeout": 1}),
    );
    let wall = t0.elapsed();
    assert!(
        wall >= Duration::from_secs(3) && wall < Duration::from_millis(4_500),
        "{wall:?}"
    );
    assert!(
        text(&r).contains(
            "[timeout after 1s: process group killed (TERM, then KILL after 2s) wall_time="
        )
    );
    // Fractional and out-of-range timeouts.
    let r = b.run("bash", json!({"command": "true", "timeout": 1.5}));
    assert_eq!(
        text(&r),
        "error: invalid_input: timeout must be an integer from 1 to 3600 (seconds)"
    );
    let r = b.run("bash", json!({"command": "true", "timeout": 0}));
    assert!(r.result.is_error);
    let r = b.run("bash", json!({"command": "true", "timeout": 120.0}));
    assert!(!r.result.is_error, "{}", text(&r));
}

#[test]
fn bash_background_jobs() {
    let b = Bench::new("bash-bg");
    // Not redirected: the group is killed after the 2 s drain grace, with the note.
    let pidfile = b.work().join("bg.pid");
    let t0 = Instant::now();
    let r = bash(
        &b,
        &format!(
            "sleep 300 & echo $! > '{}'; echo started",
            pidfile.display()
        ),
    );
    let wall = t0.elapsed();
    assert!(
        wall >= Duration::from_secs(2) && wall < Duration::from_secs(4),
        "{wall:?}"
    );
    let t = text(&r);
    assert!(t.starts_with("started\n"), "{t}");
    assert!(t.contains(super::bash::BG_NOTE));
    let pg = r.result.meta.wall_ms; // silence unused warnings in some configurations
    let _ = pg;
    // Redirected: returns at once; the job survives (killed here by the test, by its pgid).
    let t0 = Instant::now();
    let r = bash(&b, "sleep 300 > /dev/null 2>&1 & echo $$");
    assert!(t0.elapsed() < Duration::from_secs(1));
    let pgid: i32 = text(&r).lines().next().unwrap().trim().parse().unwrap();
    assert!(group_alive(pgid), "the redirected job should still run");
    assert!(kill_group(pgid, Signal::Kill));
    let t0 = Instant::now();
    while group_alive(pgid) && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!group_alive(pgid));
}

#[test]
fn bash_abort_kills_the_group() {
    let b = Bench::new("bash-abort");
    let pidfile = b.work().join("pg");
    let abort = b.abort.clone();
    let pf = pidfile.clone();
    let h = std::thread::spawn(move || {
        let t0 = Instant::now();
        while !pf.exists() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        let pg: i32 = std::fs::read_to_string(&pf)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let t = Instant::now();
        abort.trigger();
        (pg, t)
    });
    let r = bash(
        &b,
        &format!("echo $$ > '{}'; sleep 100 & sleep 100", pidfile.display()),
    );
    let (pg, t) = h.join().unwrap();
    let took = t.elapsed();
    assert!(took < Duration::from_millis(200), "{took:?}");
    assert!(text(&r).contains("[aborted after "), "{}", text(&r));
    assert!(text(&r).ends_with("s: process group killed]"));
    assert_eq!(r.summary, "aborted");
    assert!(!group_alive(pg));
    assert!(b.abort.groups().is_empty());
}

// ---------------------------------------------------------------------------------------------
// 6. run_group
// ---------------------------------------------------------------------------------------------

#[test]
fn run_group_separate_stdin_cancel() {
    let sh = |c: &str| {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(c);
        cmd
    };
    // Separate: the last 1 MiB of each stream, exact totals for a 3 MiB writer.
    let o = run_group(
        sh("head -c 3145728 /dev/zero | tr '\\0' o; head -c 3145728 /dev/zero | tr '\\0' e >&2; printf END; printf ERR >&2"),
        RunSpec {
            stdin: None,
            timeout: Duration::from_secs(30),
            cancel: None,
            on_spawn: None,
            capture: CaptureSpec::Separate { max_each: 1_048_576 },
        },
    )
    .unwrap();
    let Captured::Separate {
        stdout,
        stderr,
        stdout_total,
        stderr_total,
    } = o.output
    else {
        panic!()
    };
    assert_eq!((stdout_total, stderr_total), (3_145_731, 3_145_731));
    assert_eq!((stdout.len(), stderr.len()), (1_048_576, 1_048_576));
    assert!(stdout.ends_with(b"oooEND") && stderr.ends_with(b"eeeERR"));
    // stdin reaches cat; a child that never reads stdin does not fail the call.
    let o = run_group(
        sh("cat"),
        RunSpec {
            stdin: Some(b"piped bytes"),
            timeout: Duration::from_secs(10),
            cancel: None,
            on_spawn: None,
            capture: CaptureSpec::Separate { max_each: 1024 },
        },
    )
    .unwrap();
    let Captured::Separate { stdout, .. } = o.output else {
        panic!()
    };
    assert_eq!(stdout, b"piped bytes");
    let big = vec![b'x'; 1 << 20];
    let o = run_group(
        sh("exit 0"),
        RunSpec {
            stdin: Some(&big),
            timeout: Duration::from_secs(10),
            cancel: None,
            on_spawn: None,
            capture: CaptureSpec::Separate { max_each: 1024 },
        },
    )
    .unwrap();
    assert_eq!(o.status.unwrap().code(), Some(0));
    // Cancel during sleep 100: cancelled within 50 ms of TERM, the group empty.
    let cancel = AtomicBool::new(false);
    let pgid = std::sync::atomic::AtomicI32::new(0);
    let on_spawn = |pg: i32| {
        pgid.store(pg, Ordering::SeqCst);
        true
    };
    let t_cancel = std::sync::Mutex::new(None);
    let o = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            *t_cancel.lock().unwrap() = Some(Instant::now());
            cancel.store(true, Ordering::SeqCst);
        });
        run_group(
            sh("sleep 100"),
            RunSpec {
                stdin: None,
                timeout: Duration::from_secs(60),
                cancel: Some(&cancel),
                on_spawn: Some(&on_spawn),
                capture: CaptureSpec::Separate { max_each: 1024 },
            },
        )
        .unwrap()
    });
    let took = t_cancel.lock().unwrap().unwrap().elapsed();
    assert!(o.cancelled && !o.timed_out);
    assert!(took < Duration::from_millis(100), "{took:?}");
    assert!(!group_alive(pgid.load(Ordering::SeqCst)));
    // trap '' TERM under cancel: KILL after 2 s.
    let cancel = AtomicBool::new(true);
    let t0 = Instant::now();
    let o = run_group(
        sh("trap '' TERM; sleep 30"),
        RunSpec {
            stdin: None,
            timeout: Duration::from_secs(60),
            cancel: Some(&cancel),
            on_spawn: None,
            capture: CaptureSpec::Separate { max_each: 1024 },
        },
    )
    .unwrap();
    assert!(o.cancelled && o.killed);
    let w = t0.elapsed();
    assert!(
        w >= Duration::from_secs(2) && w < Duration::from_secs(4),
        "{w:?}"
    );
    // Merged without a spill dir: head + tail, no file.
    let o = run_group(
        sh("head -c 102400 /dev/zero | tr '\\0' 'a' | fold -w 99"),
        RunSpec {
            stdin: None,
            timeout: Duration::from_secs(10),
            cancel: None,
            on_spawn: None,
            capture: CaptureSpec::Merged {
                spill_dir: None,
                name: "x".into(),
            },
        },
    )
    .unwrap();
    let Captured::Merged {
        text,
        total,
        spill_path,
        ..
    } = o.output
    else {
        panic!()
    };
    assert!(spill_path.is_none());
    assert!(total > 100_000);
    assert!(
        text.contains("omitted; full output (") && text.contains(" bytes) not saved ...]"),
        "{}",
        &text[3900..4200]
    );
    assert!(text.len() < 31_000);
}

// ---------------------------------------------------------------------------------------------
// 5. ext
// ---------------------------------------------------------------------------------------------

/// ADR-009 (WIRE-009): boot passes the workspace as the tools' cwd (`ProcFacts.cwd` →
/// `PathCx.cwd`). Relative read/write/edit paths, bash and the ext call's workspace all land in
/// it, not in `INIT_HOME/work`; `~` and `$INIT_HOME` still resolve as before.
#[test]
fn workspace_cwd_for_relative_paths_bash_and_ext() {
    let host = Arc::new(FakeHost::default());
    let mut b = Bench::with("workspace-cwd", Some(host.clone()), &[]);
    let ws = b.home.path.join("userhome/projects/app");
    std::fs::create_dir_all(&ws).unwrap();
    b.cwd = Some(ws.clone());

    let r = b.run("write", json!({"path": "src/hello.txt", "content": "hi\n"}));
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(
        std::fs::read_to_string(ws.join("src/hello.txt")).unwrap(),
        "hi\n"
    );
    assert!(!b.work().join("src").exists());
    let r = b.run(
        "edit",
        json!({"path": "src/hello.txt", "edits": [{"old_text": "hi", "new_text": "hello"}]}),
    );
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(
        std::fs::read_to_string(ws.join("src/hello.txt")).unwrap(),
        "hello\n"
    );
    let r = b.run("read", json!({"path": "src/hello.txt"}));
    assert!(text(&r).contains("hello"), "{}", text(&r));

    let r = bash(&b, "pwd; cat src/hello.txt; echo made > made.txt");
    assert!(
        text(&r).starts_with(&format!("{}\nhello\n", ws.display())),
        "{}",
        text(&r)
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap(),
        "made\n"
    );

    let r = b.run(
        "ext",
        json!({"name": "count-lines", "input": {"path": "x"}}),
    );
    assert!(!r.result.is_error, "{}", text(&r));
    assert_eq!(host.workspace.lock().unwrap().clone().unwrap(), ws);

    let r = b.run(
        "write",
        json!({"path": "$INIT_HOME/work/note.txt", "content": "n"}),
    );
    assert!(!r.result.is_error, "{}", text(&r));
    assert!(b.work().join("note.txt").is_file());
    let r = b.run("write", json!({"path": "~/home.txt", "content": "h"}));
    assert!(!r.result.is_error, "{}", text(&r));
    assert!(b.home.path.join("userhome/home.txt").is_file());
}

#[test]
fn ext_dispatch() {
    let host = Arc::new(FakeHost::default());
    let mut b = Bench::with("ext", Some(host.clone()), &[]);
    let r = b.run(
        "ext",
        json!({"name": "count-lines", "input": {"path": "x"}}),
    );
    assert!(!r.result.is_error);
    assert_eq!(text(&r), "count-lines got {\"path\":\"x\"}");
    assert_eq!(host.workspace.lock().unwrap().clone().unwrap(), b.work());
    *host.kind.lock().unwrap() = OutcomeKind::Unknown;
    let r = b.run("ext", json!({"name": "nope", "input": {}}));
    assert_eq!(
        text(&r),
        "error: ext_unknown: no extension named \"nope\". Loaded: count-lines (Count lines.)\nhint: schemas are in $INIT_HOME/state/ext/catalog.json"
    );
    for (k, code) in [
        (OutcomeKind::InvalidInput, "invalid_input"),
        (OutcomeKind::Timeout, "timeout"),
        (OutcomeKind::Aborted, "aborted"),
        (OutcomeKind::Failed, "ext_failed"),
        (OutcomeKind::Busy, "ext_busy"),
        (OutcomeKind::Host, "ext_host"),
        (OutcomeKind::Oom, "ext_trap"),
        (OutcomeKind::StackOverflow, "ext_trap"),
        (OutcomeKind::Panic, "ext_trap"),
        (OutcomeKind::Trap, "ext_trap"),
        (OutcomeKind::Exit, "ext_trap"),
    ] {
        *host.kind.lock().unwrap() = k;
        let r = b.run("ext", json!({"name": "pty", "input": {}}));
        assert!(r.result.is_error);
        assert!(
            text(&r).starts_with(&format!("error: {code}: ext pty 0.1.0: ")),
            "{k:?}: {}",
            text(&r)
        );
    }
    // Validation never reaches the host.
    let before = host.calls.load(Ordering::SeqCst);
    let r = b.run("ext", json!({"name": "pty", "input": "text"}));
    assert_eq!(
        text(&r),
        "error: invalid_input: input: expected an object, found a string"
    );
    let r = b.run(
        "ext",
        json!({"name": "pty", "input": {"s": "x".repeat(1 << 20)}}),
    );
    assert!(
        first_line(&r).starts_with("error: invalid_input: input: 1,048,"),
        "{}",
        first_line(&r)
    );
    let r = b.run("ext", json!({"name": "", "input": {}}));
    assert_eq!(text(&r), "error: invalid_input: name: must not be empty");
    assert_eq!(host.calls.load(Ordering::SeqCst), before);
    // Safe mode: ext_disabled.
    b.mode = Mode::Safe;
    let r = b.run("ext", json!({"name": "pty", "input": {}}));
    assert_eq!(
        text(&r),
        "error: ext_disabled: extensions are disabled in safe mode"
    );
    // The stub host until WP14.
    let s = Bench::with("ext-stub", Some(Arc::new(StubHost)), &[]);
    let r = s.run("ext", json!({"name": "x", "input": {}}));
    assert!(first_line(&r).starts_with("error: ext_unknown: no extension named \"x\""));
}

#[test]
fn long_results_are_capped_and_spilled() {
    let host = Arc::new(FakeHost::default());
    let b = Bench::with("ext-cap", Some(host.clone()), &[]);
    let r = b.run(
        "ext",
        json!({"name": "big", "input": {"pad": "y\n".repeat(40_000)}}),
    );
    let t = text(&r);
    assert!(t.len() <= 51_200 + 600, "{}", t.len()); // head + tail + the marker line
    assert!(t.contains("bytes) omitted; full output ("));
    let p = r.result.meta.spill_path.clone().unwrap();
    assert!(std::fs::metadata(p).unwrap().len() > 80_000);
}

#[test]
fn panic_hook_for_tests_exists() {
    // The loop's panic isolation test uses this path; make sure read panics on it.
    let b = Bench::new("panic");
    let c = b.call("read", json!({"path": "x/__init_test_panic__"}));
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.run_call(&c)));
    assert!(res.is_err());
}
