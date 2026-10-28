//! Log tests (log-context.md §8: T-LOG-1 … T-LOG-9; T-LOG-5 and T-LOG-10 run at the context level).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use super::clock::{id_hex, parse_id};
use super::event::*;
use super::schema_check::{Validator, assert_file_valid};
use super::session::SessionLog;
use super::testutil::*;
use super::writer::{LogError, LogWriter, read_all};
use super::*;

fn display(text: &str) -> EventBody {
    EventBody::Display(DisplayEv {
        level: "info".into(),
        text: text.into(),
    })
}

fn owner(rng: &mut Rng, text: &str) -> EventBody {
    MessageEv {
        role: "user".into(),
        content: vec![Block::text(text)],
        source: Some("owner".into()),
        input_id: Some(rng.hex16()),
        ..Default::default()
    }
    .into()
}

fn assistant(calls: &[(&str, &str)], thinking: &str) -> EventBody {
    let mut content = vec![Block::Thinking {
        provider: "mock".into(),
        model: "mock-frontier-1".into(),
        text: thinking.into(),
        native: None,
    }];
    for (cid, name) in calls {
        content.push(Block::ToolUse {
            call_id: (*cid).into(),
            name: (*name).into(),
            input: obj(json!({"command": "ls"})),
            raw_arguments: Some("{\"command\": \"ls\"}".into()),
        });
    }
    EventBody::from(MessageEv {
        role: "assistant".into(),
        content,
        provider: Some("mock".into()),
        api: Some("chat_completions".into()),
        model: Some("mock-frontier-1".into()),
        response_id: Some(serde_json::Value::Null),
        stop_reason: Some(
            if calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            }
            .into(),
        ),
        stop_details: Some(serde_json::Value::Null),
        usage: Some(Box::new(UsageEv {
            input_tokens: 10,
            cache_read_tokens: 90,
            cache_write_tokens: 0,
            output_tokens: 20,
            ..Default::default()
        })),
        request: Some(Box::new(RequestMeta {
            k_ppm: Some(271_000),
            ..Default::default()
        })),
        ..Default::default()
    })
}

fn result(cid: &str, text: &str) -> EventBody {
    EventBody::ToolResult(ToolResultEv {
        call_id: cid.into(),
        name: "bash".into(),
        output: vec![Block::text(text)],
        is_error: false,
        spill_path: None,
        exit_code: Some(0),
        signal: None,
        wall_ms: Some(12),
        tokens_est: tokens_for_bytes(K0_PPM, text.len() as u64),
        synthetic: None,
    })
}

fn marker(cid: &str, mid: u64) -> EventBody {
    EventBody::ToolCall(ToolCallEv {
        call_id: cid.into(),
        name: "bash".into(),
        message_id: id_hex(mid),
        idempotency_key: None,
    })
}

/// One random event of a random type (all 20 types appear over a long run).
fn random_event(rng: &mut Rng, ix: &Index) -> (EventBody, Option<u64>) {
    let any_id = |rng: &mut Rng| ix.ents()[rng.below(ix.len() as u64) as usize].id;
    let n = rng.below(120) as usize;
    let t = rng.text(n);
    let b = match rng.below(20) {
        0 => owner(rng, &t),
        1 => assistant(&[("call_0", "bash"), ("call_1", "read")], &t),
        2 => result("call_0", &t),
        3 => marker("call_0", any_id(rng)),
        4 => EventBody::Steer(SteerEv {
            content: vec![Block::text(t)],
            input_id: rng.hex16(),
            channel: None,
        }),
        5 => EventBody::Command(CommandEv {
            input_id: rng.hex16(),
            text: "/compact".into(),
            outcome: None,
        }),
        6 => EventBody::Signal(SignalEv {
            ctx_tokens: 5,
            ctx_permille: 1,
            text: format!("[init:signals] {t}"),
            extra: obj(json!({"uptime_s": 3})),
        }),
        7 => EventBody::Notice(NoticeEv {
            source: "kernel".into(),
            code: "lkg".into(),
            projected: true,
            text: t,
            notice_id: Some(rng.hex16()),
            data: Some(json!({"a": 1})),
        }),
        8 => EventBody::Gate(GateEv {
            gate_id: "g1".into(),
            sha: SHA.into(),
            stage: "G1".into(),
            pass: Some(true),
            is_async: true,
            text: Some(t),
            notice_id: Some(rng.hex16()),
            extra: obj(json!({"score": 900, "cost": 1200})),
        }),
        9 => EventBody::UpdateFailed(UpdateFailedEv {
            sha: SHA.into(),
            stage: "G2".into(),
            reason: "x".into(),
            text: t,
            seq: Some(rng.below(1 << 40)),
            notice_id: None,
            extra: obj(json!({"from_sha": SHA, "log_tail": "tail"})),
        }),
        10 => EventBody::ThinningDone(ThinningDoneEv {
            removed: vec!["a".into()],
            kept: vec![],
            expired: vec![],
            tokens_saved: -3,
            text: t,
            notice_id: None,
        }),
        11 => EventBody::Reload(ReloadEv {
            tier: "T1".into(),
            ok: true,
            components: vec!["ext/x".into()],
            prefix_changed: false,
            head_hash_before: format!("sha256:{}", "a".repeat(64)),
            head_hash_after: format!("sha256:{}", "a".repeat(64)),
            error: None,
            text: format!("[init:reload] {t}"),
        }),
        12 => EventBody::Mask(MaskEv {
            masked: vec![MaskEntry {
                id: id_hex(any_id(rng)),
                part: "output".into(),
                tokens: 300,
                placeholder: "[init:masked] x".into(),
                call_id: None,
                input: None,
            }],
            tokens_before: 10,
            tokens_after_est: 5,
            n: 1,
        }),
        13 => EventBody::Label(LabelEv {
            target_id: id_hex(any_id(rng)),
            label: Some(t.chars().take(100).collect()),
        }),
        14 => display(&t),
        15 => EventBody::Error(ErrorEv {
            where_: "tool".into(),
            code: "x".into(),
            message: t,
            data: None,
        }),
        16 => EventBody::SelfEdit(SelfEditEv {
            tier: "T0".into(),
            sha: SHA.into(),
            extra: obj(json!({"branch": null, "trailers": {"Why": "w"}})),
        }),
        17 => {
            let target = any_id(rng);
            return (
                EventBody::Branch(BranchEv {
                    from_leaf: id_hex(target),
                    reason: "agent".into(),
                    summary: Some(t.clone()),
                    text: Some(format!("[init:branch] {t}")),
                }),
                Some(target),
            );
        }
        18 => {
            let mut b = boot_ev(&format!("[init:boot] {t}"));
            b.prefix_hash = format!("sha256:{}", "b".repeat(64));
            EventBody::Boot(Box::new(b))
        }
        _ => EventBody::Compaction(Box::new(CompactionEv {
            summary: "<summary></summary>".into(),
            text: t,
            first_kept_id: id_hex(any_id(rng)),
            through_id: id_hex(any_id(rng)),
            tokens_before: 100,
            tokens_after_est: 50,
            n: 1,
            trigger: "auto".into(),
            focus: None,
            artifacts: json!({}),
            owner_msgs: vec![],
            summarizer: Some(json!({"model": "m"})),
            retries: 0,
            repaired_ids: vec![],
        })),
    };
    (b, None)
}

/// T-LOG-1: append 10k random events, reopen: identical index, leaf and path. Every line is ICJ and
/// validates against the §4.2 JSON Schema.
#[test]
fn t_log_1_random_events_reopen() {
    let home = TempHome::new("tlog1");
    let mut rng = Rng(0x5eed_1234);
    let (index, leaf, path) = {
        let (mut log, rep) = open(&home.path, SID);
        assert_eq!(rep.created.as_deref(), Some("cold"));
        for _ in 0..10_000 {
            let (b, parent) = random_event(&mut rng, &log.index);
            match parent {
                Some(p) => log.append_with_parent(&b, p).unwrap(),
                None => log.append(&b).unwrap(),
            };
        }
        log.sync().unwrap();
        (log.index.clone(), log.leaf(), log.path())
    };
    // Reopen with the bare writer: SessionLog::open would also close the random open tool rounds.
    let o = LogWriter::open(&home.session_file(SID), SID, Duration::from_secs(2)).unwrap();
    assert_eq!(o.index.ents(), index.ents());
    assert_eq!(o.index.input_ids, index.input_ids);
    assert_eq!(o.index.notice_ids, index.notice_ids);
    assert_eq!(o.index.update_failed_seqs, index.update_failed_seqs);
    assert_eq!(o.index.labels, index.labels);
    assert_eq!(o.writer.leaf(), leaf);
    assert_eq!(o.index.path(o.writer.leaf()), path);
    assert!(o.scan.skipped.is_empty() && !o.newline_appended);
    drop(o);
    // Every line is canonical and schema-valid; every type was exercised.
    let bytes = std::fs::read(home.session_file(SID)).unwrap();
    let mut kinds = std::collections::BTreeSet::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        assert!(
            icj::is_canonical(line),
            "not ICJ: {}",
            String::from_utf8_lossy(line)
        );
        let (e, _) = Event::parse_line(line).unwrap();
        assert_ne!(e.kind(), Kind::Other, "{}", String::from_utf8_lossy(line));
        kinds.insert(e.kind());
    }
    assert_eq!(kinds.len(), 20, "{kinds:?}");
    assert_file_valid(&home.session_file(SID));
}

fn build_small_log(home: &Path) -> Vec<u64> {
    let (mut log, _) = open(home, SID);
    let mut rng = Rng(7);
    let mut ids = vec![log.leaf().unwrap()];
    ids.push(log.append(&owner(&mut rng, "hello")).unwrap());
    ids.push(
        log.append(&assistant(&[("call_0", "bash")], "think"))
            .unwrap(),
    );
    ids.push(log.append(&result("call_0", "done")).unwrap());
    ids.push(
        log.append(&display("the last line, with some length to cut into"))
            .unwrap(),
    );
    log.sync().unwrap();
    ids
}

fn count_valid(path: &Path) -> usize {
    read_all(path).unwrap().len()
}

/// Checks one damaged file: bytes kept, `\n` appended first, leaf, report, counts.
fn check_reopen(
    home: &TempHome,
    damaged_len: u64,
    expect_leaf: u64,
    expect_skipped: Option<Skipped>,
    expect_newline: bool,
) {
    let path = home.session_file(SID);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(before.len() as u64, damaged_len);
    let valid_before = count_valid(&path);
    let (mut log, rep) = open(&home.path, SID);
    assert_eq!(log.leaf(), Some(expect_leaf), "leaf");
    assert_eq!(rep.repair.newline_appended, expect_newline);
    assert_eq!(rep.repair.skipped.first().copied(), expect_skipped);
    let after_open = std::fs::read(&path).unwrap();
    assert_eq!(
        &after_open[..before.len()],
        &before[..],
        "damaged bytes stay in place"
    );
    if expect_newline {
        assert_eq!(
            after_open[before.len()],
            b'\n',
            "first appended byte is a newline"
        );
    }
    // Boot + reports, as at a real boot.
    let boot = EventBody::Boot(Box::new(boot_ev("[init:boot] after repair")));
    log.append(&boot).unwrap();
    log.append_repair_reports(&rep).unwrap();
    let appended = 1
        + usize::from(!rep.repair.skipped.is_empty())
        + usize::from(rep.skipped_before_last_valid);
    let parsed_tail = usize::from(expect_newline && expect_skipped.is_none());
    assert_eq!(
        count_valid(&path),
        valid_before + parsed_tail + appended,
        "valid_after == valid_before + appended"
    );
    let evs = read_all(&path).unwrap();
    if expect_skipped.is_some() {
        assert!(
            evs.iter()
                .any(|e| matches!(&e.body, EventBody::Error(x) if x.code == "corrupt_line"))
        );
    }
    assert_file_valid(&path);
}

/// T-LOG-2: every cut inside the last line, plus testing.md F-corrupt-tail's three variants.
#[test]
fn t_log_2_torn_tail_every_offset() {
    let base = TempHome::new("tlog2-base");
    let ids = build_small_log(&base.path);
    let full = std::fs::read(base.session_file(SID)).unwrap();
    let last_start = full[..full.len() - 1]
        .iter()
        .rposition(|b| *b == b'\n')
        .unwrap()
        + 1;
    let last_len = full.len() - last_start;
    // Cuts at every byte inside the last line (1..len-1 bytes kept), and "only the LF missing".
    for keep in 1..last_len {
        let home = TempHome::new("tlog2");
        let path = home.session_file(SID);
        std::fs::write(&path, &full[..last_start + keep]).unwrap();
        if keep == last_len - 1 {
            // The crash hit just before the terminator: the appended `\n` completes a valid event.
            check_reopen(&home, (last_start + keep) as u64, ids[4], None, true);
        } else {
            let sk = Skipped {
                offset: last_start as u64,
                len: keep as u64 + 1,
            };
            check_reopen(&home, (last_start + keep) as u64, ids[3], Some(sk), true);
        }
    }
    // F-corrupt-tail 1: `truncate -s -17`.
    let home = TempHome::new("tlog2-f1");
    std::fs::write(home.session_file(SID), &full[..full.len() - 17]).unwrap();
    let sk = Skipped {
        offset: last_start as u64,
        len: (last_len - 17) as u64 + 1,
    };
    check_reopen(&home, (full.len() - 17) as u64, ids[3], Some(sk), true);
    // F-corrupt-tail 2: append `{"id":"deadbeef` without a newline.
    let home = TempHome::new("tlog2-f2");
    let mut f2 = full.clone();
    f2.extend_from_slice(br#"{"id":"deadbeef"#);
    std::fs::write(home.session_file(SID), &f2).unwrap();
    check_reopen(
        &home,
        f2.len() as u64,
        ids[4],
        Some(Skipped {
            offset: full.len() as u64,
            len: 16,
        }),
        true,
    );
    // F-corrupt-tail 3: append the bytes 0xFF 0xFE \n (invalid UTF-8, terminated).
    let home = TempHome::new("tlog2-f3");
    let mut f3 = full.clone();
    f3.extend_from_slice(&[0xFF, 0xFE, b'\n']);
    std::fs::write(home.session_file(SID), &f3).unwrap();
    check_reopen(
        &home,
        f3.len() as u64,
        ids[4],
        Some(Skipped {
            offset: full.len() as u64,
            len: 3,
        }),
        false,
    );
}

/// Child for T-LOG-3: appends forever, printing each id after `write_all` returned.
#[test]
#[ignore]
fn child_appender() {
    let Ok(home) = std::env::var("INIT_WP12_CHILD_HOME") else {
        return;
    };
    let (mut log, _) = open(Path::new(&home), SID);
    let mut out = std::io::stdout().lock();
    for i in 0u64.. {
        let id = log
            .append(&display(&format!(
                "n{i} {}",
                "x".repeat((i % 300) as usize)
            )))
            .unwrap();
        writeln!(out, "ok {}", id_hex(id)).unwrap();
        out.flush().unwrap();
    }
}

fn child(test: &str, home: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            test,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INIT_WP12_CHILD_HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// T-LOG-3: SIGKILL a child appender at random times (500 rounds; `INIT_WP12_LOG3_ITERS` overrides).
/// Every reopen succeeds, and every event whose `write_all` returned is still valid.
#[test]
fn t_log_3_sigkill_appender() {
    let iters: u64 = std::env::var("INIT_WP12_LOG3_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let home = TempHome::new("tlog3");
    let path = home.session_file(SID);
    let mut rng = Rng(0xdead_beef);
    let mut confirmed: Vec<u64> = Vec::new();
    for round in 0..iters {
        let mut c = child("log::tests::child_appender", &home.path);
        let stdout = c.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut ids = Vec::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // The first line starts with libtest's "test … ... " prefix.
                if let Some(id) = line.rsplit_once("ok ").and_then(|(_, h)| parse_id(h)) {
                    ids.push(id);
                }
            }
            ids
        });
        std::thread::sleep(Duration::from_millis(5 + rng.below(40)));
        c.kill().unwrap(); // SIGKILL to the PID we spawned
        c.wait().unwrap();
        let ids = reader.join().unwrap();
        let opened =
            LogWriter::open(&path, SID, Duration::from_secs(2)).expect("reopen after SIGKILL");
        for id in &ids {
            assert!(
                opened.index.contains(*id),
                "round {round}: confirmed event {} lost",
                id_hex(*id)
            );
        }
        confirmed.extend(ids);
    }
    let all = read_all(&path).unwrap();
    let set: std::collections::HashSet<u64> = all.iter().map(|e| e.id).collect();
    assert!(confirmed.iter().all(|id| set.contains(id)));
    assert!(
        confirmed.len() as u64 > iters,
        "children appended too little: {}",
        confirmed.len()
    );
    eprintln!(
        "T-LOG-3: {iters} kills, {} confirmed events, {} valid events",
        confirmed.len(),
        all.len()
    );
}

/// Child for T-LOG-4: takes the lock and waits to be killed.
#[test]
#[ignore]
fn child_locker() {
    let Ok(home) = std::env::var("INIT_WP12_CHILD_HOME") else {
        return;
    };
    let (_log, _) = open(Path::new(&home), SID);
    println!("locked");
    std::io::stdout().flush().unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

/// T-LOG-4: a second process gets `WouldBlock` (→ `Locked` after its budget); after the holder dies,
/// the lock is acquired within 100 ms.
#[test]
fn t_log_4_two_process_lock() {
    let home = TempHome::new("tlog4");
    build_small_log(&home.path);
    let path = home.session_file(SID);
    let mut c = child("log::tests::child_locker", &home.path);
    let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
    loop {
        match lines.next() {
            Some(Ok(l)) if l.ends_with("locked") => break, // libtest prints "test … ... " first
            Some(Ok(_)) => continue,
            other => panic!("child ended early: {other:?}"),
        }
    }
    match LogWriter::open(&path, SID, Duration::from_millis(300)) {
        Err(LogError::Locked(s)) => assert_eq!(
            LogError::Locked(s).to_string(),
            format!("log: session {SID} locked by another writer")
        ),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("second writer got the lock"),
    }
    c.kill().unwrap();
    c.wait().unwrap();
    // The kernel released the lock when the holder died: a single try (no wait budget) gets it.
    // Exact, where the old "within 100 ms" bound also measured a loaded machine's scheduler.
    let _w = LogWriter::open(&path, SID, Duration::ZERO).unwrap();
    // In-process too: a second handle on the same file is refused (flock is per open file).
    assert!(matches!(
        LogWriter::open(&path, SID, Duration::from_millis(100)),
        Err(LogError::Locked(_))
    ));
}

/// T-LOG-6 (log side): unknown types and fields rehydrate; their bytes are preserved.
#[test]
fn t_log_6_unknown_types_preserved() {
    let home = TempHome::new("tlog6");
    let line_unknown;
    {
        let (mut log, _) = open(&home.path, SID);
        log.writer
            .append_fields(
                "wormhole",
                obj(json!({"payload": {"z": 1, "a": [true]}})),
                None,
                &mut log.index,
            )
            .unwrap();
        let bytes = std::fs::read(home.session_file(SID)).unwrap();
        line_unknown = bytes[bytes[..bytes.len() - 1]
            .iter()
            .rposition(|b| *b == b'\n')
            .unwrap()
            + 1..]
            .to_vec();
        log.append(&display("after")).unwrap();
        log.sync().unwrap();
    }
    // A known type with an unknown field, written by "a newer body".
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(home.session_file(SID))
        .unwrap();
    let last = read_all(&home.session_file(SID))
        .unwrap()
        .last()
        .unwrap()
        .id;
    let extra = format!(
        "{{\"future_field\":{{\"x\":1}},\"id\":\"00000000000000f1\",\"level\":\"info\",\"parent_id\":\"{}\",\"text\":\"t\",\"ts\":\"2026-10-19T20:30:15.123Z\",\"type\":\"display\"}}\n",
        id_hex(last)
    );
    f.write_all(extra.as_bytes()).unwrap();
    drop(f);
    let before = std::fs::read(home.session_file(SID)).unwrap();
    let (log, rep) = open(&home.path, SID);
    assert!(rep.repair.skipped.is_empty());
    assert_eq!(log.leaf(), Some(0xf1));
    assert_eq!(log.path().len(), 4);
    let kinds: Vec<Kind> = log
        .path()
        .iter()
        .map(|&p| log.index.ents()[p].kind)
        .collect();
    assert_eq!(
        kinds,
        vec![Kind::Session, Kind::Other, Kind::Display, Kind::Display]
    );
    let after = std::fs::read(home.session_file(SID)).unwrap();
    assert_eq!(after, before, "bytes preserved");
    assert!(
        after
            .windows(line_unknown.len())
            .any(|w| w == line_unknown.as_slice())
    );
}

/// T-LOG-7: a corrupt middle line: skipped, path truncated at the gap, error + display appended, no
/// truncation of the file.
#[test]
fn t_log_7_corrupt_middle_line() {
    let home = TempHome::new("tlog7");
    let ids = build_small_log(&home.path);
    let path = home.session_file(SID);
    let ix = LogWriter::open(&path, SID, Duration::from_secs(2))
        .unwrap()
        .index;
    let victim = *ix.get(ids[2]).unwrap(); // the assistant message
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_at(&vec![b'#'; victim.len as usize - 1], victim.off)
        .unwrap();
    drop(f);
    let len_before = std::fs::metadata(&path).unwrap().len();
    let (mut log, rep) = open(&home.path, SID);
    assert_eq!(
        rep.repair.skipped,
        vec![Skipped {
            offset: victim.off,
            len: victim.len as u64
        }]
    );
    assert!(rep.skipped_before_last_valid);
    // The path stops at the missing parent: it starts at the result after the gap.
    let path_ids: Vec<u64> = log.path().iter().map(|&p| log.index.ents()[p].id).collect();
    assert_eq!(path_ids, vec![ids[3], ids[4]]);
    log.append(&EventBody::Boot(Box::new(boot_ev("[init:boot] x"))))
        .unwrap();
    log.append_repair_reports(&rep).unwrap();
    let evs = read_all(&path).unwrap();
    let n = evs.len();
    assert!(
        matches!(&evs[n - 2].body, EventBody::Error(e) if e.code == "corrupt_line" && e.where_ == "log")
    );
    assert!(matches!(&evs[n - 1].body, EventBody::Display(d) if d.level == "warn"));
    assert!(
        std::fs::metadata(&path).unwrap().len() > len_before,
        "nothing truncated"
    );
    assert_file_valid(&path);
}

/// T-LOG-8: crash between the assistant message and its markers → `not_started`; between markers and
/// results → `interrupted`. Synthetic results are appended in block order, before the boot event.
#[test]
fn t_log_8_interrupted_rounds() {
    // (a) no markers at all
    let home = TempHome::new("tlog8a");
    {
        let (mut log, _) = open(&home.path, SID);
        let mut rng = Rng(3);
        log.append(&owner(&mut rng, "go")).unwrap();
        log.append(&assistant(&[("c1", "bash"), ("c2", "bash")], "t"))
            .unwrap();
        log.sync().unwrap();
    }
    let (mut log, rep) = open(&home.path, SID);
    assert_eq!((rep.not_started, rep.interrupted), (2, 0));
    log.append(&EventBody::Boot(Box::new(boot_ev("[init:boot] b"))))
        .unwrap();
    let evs = read_all(&home.session_file(SID)).unwrap();
    let n = evs.len();
    for (e, cid) in evs[n - 3..n - 1].iter().zip(["c1", "c2"]) {
        match &e.body {
            EventBody::ToolResult(r) => {
                assert_eq!(r.call_id, cid);
                assert_eq!(r.synthetic.as_deref(), Some("not_started"));
                assert!(r.is_error);
                assert_eq!(r.output, vec![Block::text(NOT_STARTED_TEXT)]);
                assert_eq!(
                    r.tokens_est,
                    tokens_for_bytes(271_000, NOT_STARTED_TEXT.len() as u64),
                    "k from request.k_ppm"
                );
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(evs[n - 1].kind(), Kind::Boot);
    // (b) markers for both, a result for c1 only
    let home = TempHome::new("tlog8b");
    {
        let (mut log, _) = open(&home.path, SID);
        let mut rng = Rng(4);
        log.append(&owner(&mut rng, "go")).unwrap();
        let m = log
            .append(&assistant(&[("c1", "bash"), ("c2", "bash")], "t"))
            .unwrap();
        log.append(&marker("c1", m)).unwrap();
        log.append(&marker("c2", m)).unwrap();
        log.append(&result("c1", "ok")).unwrap();
        log.sync().unwrap();
    }
    let (_log, rep) = open(&home.path, SID);
    assert_eq!((rep.not_started, rep.interrupted), (0, 1));
    let evs = read_all(&home.session_file(SID)).unwrap();
    match &evs.last().unwrap().body {
        EventBody::ToolResult(r) => {
            assert_eq!(r.call_id, "c2");
            assert_eq!(r.synthetic.as_deref(), Some("interrupted"));
            assert_eq!(r.output, vec![Block::text(INTERRUPTED_TEXT)]);
        }
        other => panic!("{other:?}"),
    }
    // A completed round needs nothing on the next open.
    drop(_log);
    let (_log, rep) = open(&home.path, SID);
    assert_eq!(rep.interrupted_calls(), 0);
    assert_file_valid(&home.session_file(SID));
}

/// Handoff at the writer level: unlock, a second writer appends (and tears its last line), relock and
/// rescan picks its events up and closes the torn tail.
#[test]
fn unlock_relock_rescan() {
    let home = TempHome::new("relock");
    build_small_log(&home.path);
    let (mut b0, _) = open(&home.path, SID);
    let (len, leaf) = b0.writer.unlock().unwrap();
    {
        let (mut b1, _) = open(&home.path, SID);
        b1.append(&EventBody::Boot(Box::new(boot_ev("[init:boot] b1"))))
            .unwrap();
        b1.sync().unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(home.session_file(SID))
            .unwrap();
        f.write_all(br#"{"id":"00000000000000c1","par"#).unwrap();
    }
    let rep = b0
        .writer
        .relock_and_rescan(len, Duration::from_secs(2), &mut b0.index)
        .unwrap();
    assert_eq!(rep.valid, 1);
    assert_eq!(rep.skipped.len(), 1);
    assert!(rep.tail.is_some());
    b0.writer.set_leaf(leaf);
    let id = b0.append(&display("b0 again")).unwrap();
    assert_eq!(b0.index.get(id).unwrap().parent(), leaf);
    assert_eq!(
        b0.index.len(),
        read_all(&home.session_file(SID)).unwrap().len()
    );
}

/// `/new` at the log level: a new file with `origin.reason:"new"`, pointer moved, old file untouched
/// and unlocked.
#[test]
fn new_session_moves_pointer() {
    let home = TempHome::new("newsess");
    build_small_log(&home.path);
    let old = std::fs::read(home.session_file(SID)).unwrap();
    let (mut log, _) = open(&home.path, SID);
    let sid = log.new_session(SHA).unwrap();
    assert_ne!(sid, SID);
    assert_eq!(
        writer::read_pointer(&home.path.join("state")).as_deref(),
        Some(sid.as_str())
    );
    assert_eq!(std::fs::read(home.session_file(SID)).unwrap(), old);
    let evs = read_all(&home.session_file(&sid)).unwrap();
    assert!(
        matches!(&evs[0].body, EventBody::Session(s) if s.origin.reason == "new" && s.origin.from_session.as_deref() == Some(SID))
    );
    // The old file is no longer locked by us.
    assert!(LogWriter::open(&home.session_file(SID), SID, Duration::from_millis(100)).is_ok());
    assert_file_valid(&home.session_file(&sid));
}

/// An empty `INIT_SESSION` generates `YYYYMMDD-HHMMSS-<6 hex>`; a missing file for a given id is
/// created `cold` with a warning; an existing empty file gets a header.
#[test]
fn session_ids_and_creation() {
    let home = TempHome::new("sids");
    let (log, rep) = open(&home.path, "");
    assert_eq!(rep.created.as_deref(), Some("cold"));
    assert!(rep.warnings.is_empty());
    assert_eq!(log.sid().len(), 22);
    assert_eq!(
        writer::read_pointer(&home.path.join("state")).as_deref(),
        Some(log.sid())
    );
    drop(log);
    let (_log, rep) = open(&home.path, "given-id");
    assert_eq!(rep.warnings.len(), 1);
    std::fs::write(home.session_file("empty"), b"").unwrap();
    let (log, _) = open(&home.path, "empty");
    assert_eq!(log.index.len(), 1);
    assert_eq!(log.index.ents()[0].kind, Kind::Session);
    assert!(
        SessionLog::open(&OpenParams {
            home: &home.path,
            env_session: "bad/id",
            sha: SHA,
            lock_wait: LOCK_WAIT
        })
        .is_err()
    );
}

/// The schema rejects what the writer must never produce, and accepts the doc's examples.
#[test]
fn schema_examples() {
    let v = Validator::new();
    let examples = [
        json!({"format":1,"id":"0b6f1c2d3e4f5061","origin":{"reason":"cold"},"parent_id":null,"session_id":"20261019-203015-9f3a2c","sha":SHA,"ts":"2026-10-19T20:30:15.123Z","type":"session"}),
        json!({"call_id":"call_0","id":"77aa000000000001","message_id":"5c1e000000000001","name":"bash","parent_id":"5c1e000000000001","ts":"2026-10-19T20:30:15.123Z","type":"tool_call"}),
        json!({"call_id":"call_0","exit_code":101,"id":"8d02000000000001","is_error":false,"name":"bash","output":[{"text":"…test result: FAILED…","type":"text"}],"parent_id":"77aa000000000001","spill_path":null,"tokens_est":1812,"ts":"2026-10-19T20:30:15.123Z","type":"tool_result","wall_ms":5310}),
    ];
    for e in &examples {
        assert_eq!(v.validate(e), Vec::<String>::new(), "{e}");
    }
    let bad = json!({"id":"8d02000000000001","parent_id":null,"ts":"2026-10-19T20:30:15.123Z","type":"tool_result","call_id":"c","name":"bash","output":[],"is_error":false});
    assert!(!v.validate(&bad).is_empty(), "tokens_est is required");
}

/// T-LOG-9: `sync_data` latency (1,000 × 1 KB appends). Measurement only; run with `--ignored`.
#[test]
#[ignore]
fn t_log_9_sync_latency() {
    let home = TempHome::new("tlog9");
    let (mut log, _) = open(&home.path, SID);
    let text = "x".repeat(1000);
    let mut ms: Vec<f64> = Vec::with_capacity(1000);
    for _ in 0..1000 {
        log.append(&display(&text)).unwrap();
        let t = Instant::now();
        log.sync().unwrap();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| ms[((ms.len() as f64 - 1.0) * q).round() as usize];
    eprintln!(
        "T-LOG-9 sync_data: p50 {:.2} ms, p90 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        p(0.5),
        p(0.9),
        p(0.99),
        ms[ms.len() - 1]
    );
    if p(0.99) > 50.0 {
        eprintln!("T-LOG-9 FLAG: p99 above 50 ms");
    }
}

/// Blobs are content-addressed and written once; the renderer reads them back as data URLs.
#[test]
fn blobs_content_addressed() {
    let home = TempHome::new("blobs");
    let a = session::store_blob(&home.path, "image/png", b"\x89PNG fake").unwrap();
    let b = session::store_blob(&home.path, "image/png", b"\x89PNG fake").unwrap();
    assert_eq!(a, b);
    assert_eq!(a, crate::sha256::tagged(b"\x89PNG fake"));
    let hex = a.strip_prefix("sha256:").unwrap();
    let p = home.path.join("state/blobs").join(format!("{hex}.png"));
    assert_eq!(std::fs::read(p).unwrap(), b"\x89PNG fake");
    let target = crate::context::Target::deepseek_flash();
    let r = crate::context::render::Renderer {
        target: &target,
        home: &home.path,
    };
    let m = crate::context::project::Msg::User {
        blocks: vec![
            Block::text("look"),
            Block::Image {
                media_type: "image/png".into(),
                blob: a,
                path: None,
                tokens_est: 1600,
            },
        ],
        tokens: 0,
    };
    let v = &r.chat_messages(&m)[0];
    assert_eq!(v["content"][0], json!({"text": "look", "type": "text"}));
    assert_eq!(
        v["content"][1]["image_url"]["url"],
        format!(
            "data:image/png;base64,{}",
            crate::context::render::base64(b"\x89PNG fake")
        )
    );
}
