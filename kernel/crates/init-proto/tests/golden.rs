//! IC-1 and testing.md §3.2 (`init-proto`): every example frame and file in the docs, plus one
//! hand-written example per command and message the docs show no example for, round-trips
//! through the typed structs and comes back byte-equal in ICJ (sorted keys, integers only).
//!
//! `fixtures/doc-examples.tsv` holds the doc examples verbatim (`<kind>\t<json>`), extracted
//! from kernel/initctl.md §4.1/§4.4, kernel/supervise.md §4.3–§4.7, kernel/sandbox.md §4.2,
//! root/root.md §4.1.3–§4.1.4/§4.5 and root/egress.md §4.2 on 2026-10-20.

use init_proto::envelope::{Event, LinkFrame};
use init_proto::files::{self, *};
use init_proto::icj;
use init_proto::initctl::*;
use init_proto::root_link::*;
use init_proto::stdio;
use init_proto::{Frame, Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn line(s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(b'\n');
    b
}

/// Parse `json` as `T`, re-encode as ICJ, and compare with the canonical form of `json`.
fn typed<T: DeserializeOwned + Serialize>(json: &str) -> T {
    let x: T = icj::from_slice(json.as_bytes()).unwrap_or_else(|e| panic!("parse {json}: {e}"));
    let back = icj::to_vec(&x).unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), String::from_utf8(icj::canonicalize(json.as_bytes()).unwrap()).unwrap());
    x
}

fn request(json: &str) -> Command {
    let r = Request::parse_line(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e:?}"));
    let c = Command::from_request(&r).unwrap_or_else(|e| panic!("{json}: {e}"));
    assert_eq!(String::from_utf8(c.to_request(r.id.clone()).to_line()).unwrap(), String::from_utf8(line(json)).unwrap());
    c
}

fn response<T: DeserializeOwned + Serialize>(json: &str) -> T {
    let Frame::Final(r) = Frame::parse_line(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e}")) else {
        panic!("not a final response: {json}")
    };
    let t: T = r.result_as().unwrap_or_else(|e| panic!("{json}: {e}"));
    let again = Response::ok(r.id.clone().unwrap(), &t).unwrap();
    assert_eq!(String::from_utf8(again.to_line()).unwrap(), String::from_utf8(line(json)).unwrap());
    t
}

fn error_response(json: &str) -> Response {
    let Frame::Final(r) = Frame::parse_line(json.as_bytes()).unwrap() else { panic!() };
    assert!(!r.is_ok());
    let e = r.result.clone().unwrap_err();
    let again = Response::err(r.id.clone(), e);
    assert_eq!(again.to_line(), line(json));
    r
}

fn progress(json: &str) -> Progress {
    let Frame::Progress(p) = Frame::parse_line(json.as_bytes()).unwrap() else { panic!("not progress: {json}") };
    assert_eq!(p.to_line(), line(json));
    p.progress
}

fn file<T: DeserializeOwned + Serialize>(json: &str) -> T {
    let x: T = files::decode(json.as_bytes()).unwrap_or_else(|e| panic!("{json}: {e}"));
    assert_eq!(String::from_utf8(files::encode(&x).unwrap()).unwrap(), String::from_utf8(line(json)).unwrap());
    x
}

#[test]
fn ic1_doc_examples_round_trip_byte_equal() {
    let fixtures = include_str!("fixtures/doc-examples.tsv");
    let mut n = 0;
    for l in fixtures.lines().filter(|l| !l.is_empty()) {
        let (kind, json) = l.split_once('\t').unwrap();
        n += 1;
        match kind {
            "initctl.request" => {
                request(json);
            }
            "initctl.response.promote" => {
                response::<PromoteResult>(json);
            }
            "initctl.response.error" => {
                error_response(json);
            }
            "initctl.progress" => {
                progress(json);
            }
            "initctl.response.ping" => {
                response::<PingResult>(json);
            }
            "initctl.response.status" => {
                let s: StatusResult = response(json);
                assert_eq!(s.root["level"], 0);
            }
            "initctl.response.level" => {
                response::<LevelResult>(json);
            }
            "initctl.response.reload" => {
                response::<ReloadResult>(json);
            }
            "initctl.response.gate" => {
                let g: GateResult = response(json);
                assert!(!g.pass);
                assert_eq!(g.stages[1].code.as_deref(), Some("gate.g1.self_json_drift"));
            }
            "initctl.response.rollback" => {
                response::<RollbackResult>(json);
            }
            "initctl.response.vendor" => {
                response::<VendorResult>(json);
            }
            "initctl.job_record" => {
                let j: JobRecord = file(json);
                assert_eq!(j.state, JobState::Running);
            }
            "file.lkg" => {
                file::<LkgFile>(json);
            }
            "file.boot" => {
                let b: BootFile = file(json);
                assert_eq!(b.env.sandbox.kind, "seatbelt");
            }
            "file.update_failed" => {
                file::<UpdateFailedFile>(json);
            }
            "file.supervisor" => {
                let s: SupervisorFile = file(json);
                assert_eq!(s.instances[0].kind, InstanceKind::Body);
            }
            "file.ready" => {
                file::<ReadyFile>(json);
            }
            "file.heartbeat" => {
                file::<HeartbeatFile>(json);
            }
            "file.lineage_local" => {
                file::<LineageLocalRecord>(json);
            }
            "file.sandbox" => {
                typed::<SandboxStatus>(json);
            }
            "stdio.to_body" => {
                let f = stdio::decode_to_body(json.as_bytes()).unwrap();
                assert_ne!(f, stdio::ToBody::Unknown);
                assert_eq!(stdio::encode_to_body(&f).unwrap(), line(json));
            }
            "stdio.from_body" => {
                let f = stdio::decode_from_body(json.as_bytes()).unwrap();
                assert_ne!(f, stdio::FromBody::Unknown);
                assert_eq!(stdio::encode_from_body(&f).unwrap(), line(json));
            }
            "link.boot_block" => {
                let b: BootBlock = typed(json);
                assert_eq!(b.boot.reason, ZoneBootReason::Cold);
            }
            "link.fd_tag" => {
                typed::<FdTag>(json);
            }
            "link.response.spawn" => {
                let LinkFrame::Response(r) = LinkFrame::parse_line(json.as_bytes()).unwrap() else { panic!() };
                let s: SpawnResult = r.result_as().unwrap();
                assert_eq!(Response::ok("k-42", &s).unwrap().to_line(), line(json));
            }
            "link.event.root" => {
                let LinkFrame::Event(e) = LinkFrame::parse_line(json.as_bytes()).unwrap() else { panic!() };
                let ev = RootEvent::from_event(&e).unwrap();
                assert!(matches!(ev, RootEvent::Exited(_)));
                assert_eq!(icj::to_line(&ev.to_event().unwrap()).unwrap(), line(json));
            }
            "link.ledger" => {
                let l: Ledger = typed(json);
                assert_eq!(l.next.unwrap().criteria.len(), 4);
            }
            "link.ceilings" => {
                // root/egress.md's example is shown with unsorted keys; compare canonical forms.
                let c: Ceilings = typed(json);
                assert_eq!(c.money.unwrap().day_limit_musd, 5_000_000);
            }
            other => panic!("unknown fixture kind {other}"),
        }
    }
    assert_eq!(n, 46);
}

const SHA: &str = "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807";

#[test]
fn every_command_request_round_trips() {
    // One canonical request per command (kernel/initctl.md §4.1 args).
    let reqs = [
        r#"{"args":{},"cmd":"status","id":"a1","v":1}"#.to_string(),
        r#"{"args":{"wait":true},"cmd":"reload","id":"a2","v":1}"#.to_string(),
        format!(r#"{{"args":{{"sha":"{SHA}","wait":true}},"cmd":"promote","id":"a3","v":1}}"#),
        r#"{"args":{"crate":"vt100","version":"0.16.2","wait":false,"workspace":"ext"},"cmd":"vendor","id":"a4","v":1}"#.to_string(),
        r#"{"args":{},"cmd":"task.template","id":"a5","v":1}"#.to_string(),
        r#"{"args":{"dir":"work/tasks/fam","wait":false},"cmd":"task.submit","id":"a6","v":1}"#.to_string(),
        r#"{"args":{},"cmd":"task.list","id":"a7","v":1}"#.to_string(),
        r#"{"args":{"family":"fam"},"cmd":"task.show","id":"a8","v":1}"#.to_string(),
        r#"{"args":{"set":{"evolution_share":"0.5","gate_usd":"2.00","promotes_day":10}},"cmd":"budget.set","id":"a9","v":1}"#.to_string(),
        r#"{"args":{"model":"deepseek/deepseek-v4-pro"},"cmd":"model.set","id":"b1","v":1}"#.to_string(),
        r#"{"args":{},"cmd":"fleet.list","id":"b2","v":1}"#.to_string(),
        format!(r#"{{"args":{{"budget_share_musd_day":500000,"parent":"{SHA}","prompt":"try a pty tool"}},"cmd":"fleet.spawn","id":"b3","v":1}}"#),
        r#"{"args":{"bid":"b1","text":"focus on tests"},"cmd":"fleet.send","id":"b4","v":1}"#.to_string(),
        r#"{"args":{"bid":"all","reason":"campaign over"},"cmd":"fleet.stop","id":"b5","v":1}"#.to_string(),
        r#"{"args":{"patch":"/h/work/k.diff","rationale":"faster gate","title":"gate: cache G0"},"cmd":"kernel.propose","id":"b6","v":1}"#.to_string(),
        r#"{"args":{"ref":"kevo/gate-cache","wait":true},"cmd":"kernel.promote","id":"b7","v":1}"#.to_string(),
        r#"{"args":{},"cmd":"kernel.status","id":"b8","v":1}"#.to_string(),
        r#"{"args":{"job":"j-000047"},"cmd":"job.status","id":"b9","v":1}"#.to_string(),
        r#"{"args":{"cmd":"promote","last":true,"timeout_s":30},"cmd":"job.wait","id":"c1","v":1}"#.to_string(),
        r#"{"args":{"limit":5,"state":"all"},"cmd":"job.list","id":"c2","v":1}"#.to_string(),
        r#"{"args":{"job":"j-000047"},"cmd":"job.cancel","id":"c3","v":1}"#.to_string(),
        r#"{"args":{"all":true},"cmd":"evo.frictions","id":"d1","v":1}"#.to_string(),
        r#"{"args":{"file":"/h/work/p.json"},"cmd":"evo.propose","id":"d2","v":1}"#.to_string(),
        r#"{"args":{"k":2,"patch":"/h/work/v2.diff","proposal":"p-000007"},"cmd":"evo.variant","id":"d3","v":1}"#.to_string(),
        r#"{"args":{"proposal":"p-000007","wait":true},"cmd":"evo.screen","id":"d4","v":1}"#.to_string(),
        r#"{"args":{},"cmd":"evo.status","id":"d5","v":1}"#.to_string(),
        r#"{"args":{"proposal":"p-000007","reason":"keep_base"},"cmd":"evo.abandon","id":"d6","v":1}"#.to_string(),
        r#"{"args":{"duration_h":48,"kind":"extend.open_groups","value":6,"why":"five tools for one task"},"cmd":"grant","id":"e1","v":1}"#.to_string(),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for r in &reqs {
        seen.insert(request(r).name());
    }
    // Together with the doc examples (ping, level, gate, promote, rollback, vendor, reload),
    // every command is covered.
    for c in [CommandName::Ping, CommandName::Level, CommandName::Gate, CommandName::Rollback] {
        seen.insert(c);
    }
    assert_eq!(seen.len(), CommandName::ALL.len());
    // Aliases parse to the canonical name.
    let r = Request::parse_line(br#"{"args":{"job":"j-000047"},"cmd":"wait","id":"x","v":1}"#).unwrap();
    assert_eq!(Command::from_request(&r).unwrap().to_request("x").cmd, "job.wait");
    let r = Request::parse_line(br#"{"args":{"patch":"p","rationale":"","title":"t"},"cmd":"propose-kernel","id":"x","v":1}"#).unwrap();
    assert_eq!(Command::from_request(&r).unwrap().name(), CommandName::KernelPropose);
}

#[test]
fn every_command_result_round_trips() {
    let rec = format!(
        r#"{{"args":{{"ref":"kevo/gate-cache"}},"cmd":"kernel.promote","created_at":"2026-10-20T08:00:00.000Z","error":{{"code":"E_GATE_FAILED","data":{{"reason":"mutation suite","report":null,"stage":"gate_of_gates"}},"details":[],"message":"gate-of-gates failed"}},"finished_at":"2026-10-20T09:00:00.000Z","id":"j-000050","lane":"live","progress":null,"reason":"gog_fail","result":null,"root_job":"s-000003","sha":"{SHA}","stages":[{{"pass":true,"stage":"K0","wall_ms":91000}}],"state":"failed","v":1}}"#
    );
    let timed_out = r#"{"args":{"ref":"evo/x"},"cmd":"promote","created_at":"t0","error":null,"finished_at":null,"id":"j-000051","lane":"member","progress":{"eta_s":null,"msg":"queued","pct":0,"stage":null,"state":"queued","ts":"t1"},"result":null,"sha":null,"stages":[],"state":"queued","timed_out":true,"v":1}"#;
    response::<JobRecord>(&format!(r#"{{"id":"1","ok":true,"result":{rec},"v":1}}"#));
    response::<JobRecord>(&format!(r#"{{"id":"1","ok":true,"result":{timed_out},"v":1}}"#));
    response::<JobListResult>(&format!(r#"{{"id":"1","ok":true,"result":{{"jobs":[{rec},{timed_out}]}},"v":1}}"#));
    let ok = |result: &str| format!(r#"{{"id":"1","ok":true,"result":{result},"v":1}}"#);
    response::<ReloadOutcome>(&ok(r#"{"components":["count-lines"],"error":null,"ok":true,"prefix_changed":false}"#));
    response::<PromoteResult>(&ok(&format!(
        r#"{{"job":"j-000047","sha":"{SHA}","stages":[{{"pass":true,"stage":"G0","wall_ms":31800}}],"state":"awaiting_boundary"}}"#
    )));
    response::<KernelProposeResult>(&ok(r#"{"path":"state/kernel/proposals/kp-0007/","proposal":"kp-0007","root_id":"p-12","status":"filed"}"#));
    response::<KernelPromoteResult>(&ok(&format!(r#"{{"job":"j-000050","sha":"{SHA}","state":"queued"}}"#)));
    response::<KernelPromoteResult>(&ok(&format!(
        r#"{{"job":"j-000050","root_job":"s-000003","sha":"{SHA}","stages":[{{"pass":true,"stage":"K3","wall_ms":5}}],"state":"awaiting_swap"}}"#
    )));
    response::<KernelStatusResult>(&ok(&format!(
        r#"{{"candidate":{{"job":"j-000050","root_job":null,"sha":"{SHA}","stages":[],"state":"running"}},"last_audit":{{"at":"2026-10-20T07:00:00.000Z","regression":false}},"running":{{"built_from":"a1c4","source":"maintainer","version":"0.1.0"}}}}"#
    )));
    response::<KernelStatusResult>(&ok(r#"{"candidate":null,"last_audit":null,"running":{"built_from":"a1c4","source":"agent","version":"0.1.1"}}"#));
    response::<TaskSubmitResult>(&ok(
        r#"{"admitted":true,"checks":[{"detail":"4 instances","ok":true,"rule":"family_size"}],"family":"fam","job":"j-000060","sealed":2,"seq":1290,"visible":["fam-1","fam-3"]}"#,
    ));
    response::<TaskSubmitResult>(&ok(
        r#"{"admitted":false,"checks":[{"detail":"1 instance; families need 2","ok":false,"rule":"family_size"}],"family":"fam","job":"j-000061","sealed":0,"seq":null,"visible":[]}"#,
    ));
    response::<TaskListResult>(&ok(
        r#"[{"admitted_at":"2026-10-20T08:00:00.000Z","eligible_from_lkg_seq":12,"family":"fam","sealed":2,"seq":1290,"size":4,"suspended":null}]"#,
    ));
    response::<BudgetSetResult>(&ok(r#"{"budgets":{"day_musd":4000000,"gate_musd":2000000}}"#));
    response::<ModelSetResult>(&ok(r#"{"model":"deepseek/deepseek-v4-pro","thinning_job":"j-000070"}"#));
    response::<FleetListResult>(&ok(&format!(
        r#"{{"candidates":[{{"cmp_permille":620,"kind":"qualified","n_f":1,"n_s":3,"sha":"{SHA}"}}],"members":[{{"bid":"b1","busy":true,"line":"{SHA}","qualified":0,"share_musd_day":500000,"spent_today_musd":120000,"submissions":1}}],"suggested_parent":"{SHA}"}}"#
    )));
    response::<FleetSpawnResult>(&ok(r#"{"bid":"b2","job":"j-000071"}"#));
    response::<FleetSendResult>(&ok(r#"{"queued":true}"#));
    response::<FleetStopResult>(&ok(r#"{"stopping":["b1","b2"]}"#));
    // A status with a fleet, a sandbox object and a handoff in progress.
    let st = r#"{"body":{"id":"b0","lane":"live"},"budget":{"day_cap_musd":5000000,"day_spent_musd":0,"gate_cap_musd":null,"gate_spent_musd":null,"soft":{"day_musd":4000000}},"cache_hit_permille":null,"constitution_sha256":"sha256:00","counters":{"crashes_total":0,"restarts_since_lkg":0,"rollbacks":0,"swaps_failed":0,"swaps_ok":0},"current":{"depth":0,"provisional":false,"sha":"s","since":"t"},"disk":{"free_mib":1},"evolution":{},"fleet":{"campaign":"c-1","members":[{"bid":"b1","busy":false,"line":"s","qualified":1,"share_musd_day":500000,"slot":"running","spent_today_musd":0,"submissions":2}],"parallel_bodies":4},"handoff":{"phase":"spawned"},"jobs":[{"cmd":"promote","id":"j-000001","stage":null,"state":"queued"}],"kernel":{"proto":1,"uptime_s":1,"version":"0.1.0"},"lane":"live","link":{"boot_reason":"cold","ceilings_age_s":null,"connected":false,"outbox_unanswered":3},"lkg":{"seq":1,"sha":"s","since":"t"},"mode":"normal","root":{"level":2},"safe":null,"sandbox":{"hidden":[],"kind":"bwrap","landlock":"full","landlock_abi":6,"network":"kernel-sockets-only","read_only":[],"seccomp":true,"sweep":"degraded","write":[],"zone":{"launcher":"root","network":"root-egress-only"}},"slot":"handoff","sweep":"degraded"}"#;
    response::<StatusResult>(&ok(st));
    // Errors with typed data.
    error_response(r#"{"error":{"code":"E_LEVEL","data":{"have":0,"need":1},"details":[],"message":"needs autonomy level L1; the level is L0"},"id":"9","ok":false,"v":1}"#);
    error_response(r#"{"error":{"code":"E_CEILING","data":{"ceiling":"parallel_bodies","limit":2,"used":2},"details":[],"message":"m"},"id":"9","ok":false,"v":1}"#);
    error_response(r#"{"error":{"code":"E_QUOTA","data":{"limit":50,"quota":"fleet_spawns_day","resets_at":"2026-10-21T00:00:00.000Z"},"details":[],"message":"m"},"id":"9","ok":false,"v":1}"#);
    error_response(r#"{"error":{"code":"E_ROOT","data":{"reason":"refused","root_code":"root.quota"},"details":[],"message":"m"},"id":"9","ok":false,"v":1}"#);
}

#[test]
fn every_stdio_frame_round_trips() {
    let to_body = [
        r#"{"id":"9c41e07a2b5d13f0","mode":"follow_up","seq":19,"t":"input","text":"later","ts":"t","v":1}"#,
        r#"{"t":"interrupt","v":1}"#,
        r#"{"t":"resume","token":"5f0d22aa91c3e7b4","update_failed":null,"v":1}"#,
        r#"{"t":"resume","token":"5f0d","update_failed":{"at":"t","exit_code":null,"from_sha":null,"job":"j-000043","log_path":null,"log_tail":"","reason":"fence","report":null,"seq":3,"sha":"s","signal":null,"stage":"fence","v":1},"v":1}"#,
        r#"{"grace_ms":30000,"reason":"root_gone","t":"stop","v":1}"#,
        r#"{"job":"j-000045","kind":"request","t":"reload","v":1}"#,
        r#"{"components":["count-lines"],"job":"j-000046","kind":"kept","t":"reload","v":1}"#,
        r#"{"data":{"cap":1,"level":1,"trigger":"unlock"},"id":"n-1","kind":"level","t":"notice","v":1}"#,
    ];
    for f in to_body {
        let x = stdio::decode_to_body(f.as_bytes()).unwrap();
        assert_eq!(stdio::encode_to_body(&x).unwrap(), line(f), "{f}");
    }
    let from_body = [
        r#"{"id":"n-1","t":"notice_ack","v":1}"#,
        r#"{"busy":false,"t":"state","v":1}"#,
        r#"{"components":[],"error":"compile error","job":"j-000045","ok":false,"prefix_changed":false,"t":"reload_done","v":1}"#,
        r#"{"kind":"turn_end","t":"display","text":"","v":1}"#,
    ];
    for f in from_body {
        let x = stdio::decode_from_body(f.as_bytes()).unwrap();
        assert_eq!(stdio::encode_from_body(&x).unwrap(), line(f), "{f}");
    }
}

#[test]
fn every_link_message_round_trips() {
    let kernel_reqs = [
        r#"{"args":{"kernel_version":"0.1.0","proto":1},"cmd":"hello","id":"k-1","v":1}"#,
        r#"{"args":{},"cmd":"ready","id":"k-2","v":1}"#,
        r#"{"args":{"args":["--manifest"],"cwd":"/h/work","env":{"INIT_HOME":"/h"},"kind":"body","lane":"live","params":{"BODY":"/h/body"},"profile":"body","program":"/h/bin/init-body.staged","replaces":"7c2e91d04b3a"},"cmd":"spawn","id":"k-42","v":1}"#,
        r#"{"args":{"instance":"7c2e91d04b3a","mode":"kill"},"cmd":"kill","id":"k-3","v":1}"#,
        r#"{"args":{},"cmd":"instances","id":"k-4","v":1}"#,
        r#"{"args":{"bytes":1024,"job":"j-000042","kind":"init-body","sha256":"5c1f"},"cmd":"store.put","id":"k-5","v":1}"#,
        r#"{"args":{"sha256":"5c1f"},"cmd":"store.get","id":"k-6","v":1}"#,
        r#"{"args":{},"cmd":"tuple.get","id":"k-7","v":1}"#,
        r#"{"args":{"data":{"from":"0d3e","job":"j-000043","to":"4b1e"},"idem":"e19a04c7b2d65f38:311","kind":"handoff"},"cmd":"lineage.append","id":"k-8","v":1}"#,
        r#"{"args":{},"cmd":"ledger.get","id":"k-9","v":1}"#,
        r#"{"args":{},"cmd":"ceilings.get","id":"k-10","v":1}"#,
        r#"{"args":{"init_kernel":"e3b0","initctl":"9a07","report":{"K0":{"pass":true}},"source_repo":"/h/lineage/kernel-cand.git","source_sha":"a1c4","version":"0.1.1"},"cmd":"system.swap","id":"k-11","v":1}"#,
        r#"{"args":{"text":"why + patch","title":"gate: cache G0"},"cmd":"proposal.file","id":"k-12","v":1}"#,
        r#"{"args":{"family":"fam","seed":7},"cmd":"sealed.family_eval","id":"k-13","v":1}"#,
        r#"{"args":{},"cmd":"sealed.holdout_rotate","id":"k-14","v":1}"#,
        r#"{"args":{"code":"body_safe","level":"alert","message":"crash loop on LKG"},"cmd":"alert","id":"k-15","v":1}"#,
    ];
    for r in kernel_reqs {
        let LinkFrame::Request(q) = LinkFrame::parse_line(r.as_bytes()).unwrap() else { panic!("{r}") };
        let k = KernelRequest::parse(&q.cmd, &q.args).unwrap_or_else(|e| panic!("{r}: {e}"));
        let n: u64 = q.id.trim_start_matches("k-").parse().unwrap();
        assert_eq!(String::from_utf8(k.to_request(n).to_line()).unwrap(), String::from_utf8(line(r)).unwrap());
    }
    let root_reqs = [
        r#"{"args":{"grace_ms":30000,"reason":"owner_stop"},"cmd":"stop","id":"r-1","v":1}"#,
        r#"{"args":{"deadline_s":10,"job":"s-000003"},"cmd":"prepare_swap","id":"r-2","v":1}"#,
        r#"{"args":{"frame":{"t":"line","text":"/status"},"qid":"q-17"},"cmd":"owner.in","id":"r-3","v":1}"#,
        r#"{"args":{"frame":{"t":"interrupt"},"qid":"q-18"},"cmd":"owner.in","id":"r-4","v":1}"#,
    ];
    for r in root_reqs {
        let LinkFrame::Request(q) = LinkFrame::parse_line(r.as_bytes()).unwrap() else { panic!("{r}") };
        let k = RootRequest::parse(&q.cmd, &q.args).unwrap_or_else(|e| panic!("{r}: {e}"));
        let n: u64 = q.id.trim_start_matches("r-").parse().unwrap();
        assert_eq!(k.to_request(n).to_line(), line(r));
    }
    let root_events = [
        r#"{"data":{"cap":3,"cause":"demotion","evolution_share_max_permille":null,"from":2,"r4_seq":400,"to":1,"trigger":{"kind":"audit","seq":399}},"ev":"level","v":1}"#,
        r#"{"data":{"mode":"safe"},"ev":"mode","v":1}"#,
        r#"{"data":{"instances":{"active":3,"limit":12},"parallel_bodies":{"active":1,"limit":1}},"ev":"ceilings","v":1}"#,
        r#"{"data":{"job":"s-000003","msg":"mutation suite passed","stage":"gog_pass"},"ev":"swap","v":1}"#,
    ];
    for r in root_events {
        let LinkFrame::Event(e) = LinkFrame::parse_line(r.as_bytes()).unwrap() else { panic!("{r}") };
        let ev = RootEvent::from_event(&e).unwrap();
        assert!(!matches!(ev, RootEvent::Unknown(_)), "{r}");
        assert_eq!(icj::to_line(&ev.to_event().unwrap()).unwrap(), line(r));
    }
    let kernel_events = [
        r#"{"data":{"busy":true,"mode":"normal","sha12":"4b1e0c2d9a7f","slot":"running"},"ev":"hb","v":1}"#,
        r#"{"data":{"frame":{"boot_seq":57,"call_id":"c1","kind":"tool_start","name":"bash","t":"display","text":"cat SELF.md"}},"ev":"owner.out","v":1}"#,
        r#"{"data":{"frame":{"seq":18,"t":"echo","text":"add a pty tool"}},"ev":"owner.out","v":1}"#,
        r#"{"data":{"frame":{"level":"warn","t":"notice","text":"G4 queued","ts":"t"}},"ev":"owner.out","v":1}"#,
        r#"{"data":{"frame":{"busy":false,"level":0,"members":0,"mode":"normal","model":"deepseek-flash","queued":0,"sha12":"4b1e0c2d9a7f","slot":"running","t":"state","task_musd":12000}},"ev":"owner.out","v":1}"#,
        r#"{"data":{"frame":{"busy":false,"level":0,"members":0,"mode":"normal","model":null,"queued":0,"sha12":"4b1e0c2d9a7f","slot":"running","t":"state","task_musd":null}},"ev":"owner.out","v":1}"#,
        // tui.md §9 Q2: fields the body adds to a display frame are relayed unchanged.
        r#"{"data":{"frame":{"boot_seq":57,"call_id":"c1","detail":{"lines":96},"kind":"tool_end","name":"bash","ok":true,"t":"display","text":"exit 0"}},"ev":"owner.out","v":1}"#,
    ];
    for r in kernel_events {
        let e: Event = serde_json::from_str(r).unwrap();
        let ev = KernelEvent::from_event(&e).unwrap_or_else(|x| panic!("{r}: {x}"));
        assert_eq!(icj::to_line(&ev.to_event().unwrap()).unwrap(), line(r));
    }
    // Results.
    typed::<LineageAppendResult>(r#"{"dup":false,"hash":"sha256:5d2a","seq":311}"#);
    typed::<Vec<InstanceInfo>>(r#"[{"instance":"7c2e91d04b3a","kind":"body","lane":"live","pid":48121,"started_at":"t"}]"#);
    typed::<KillResult>(r#"{"processes":4}"#);
    typed::<StorePutResult>(r#"{"sha256":"5c1f"}"#);
    typed::<StoreGetResult>(r#"{"bytes":1024}"#);
    typed::<TupleGetResult>(r#"{"audited":"t3f09c1e5b7d2","current":{"id":"t9a07c2e91d04"},"lkg":"t9a07c2e91d04"}"#);
    typed::<SystemSwapResult>(r#"{"job":"s-000003"}"#);
    typed::<ProposalFileResult>(r#"{"id":"p-12"}"#);
    typed::<HoldoutRotateResult>(r#"{"count":3}"#);
    // An unknown instance kind from a newer Root is kept, not refused.
    let i: InstanceInfo = typed(r#"{"instance":"x","kind":"sealed_eval","lane":null,"pid":1,"started_at":"t"}"#);
    assert_eq!(i.kind, InstanceKind::Other("sealed_eval".into()));
}

#[test]
fn version_mismatch_errors_everywhere() {
    use init_proto::icj::UNSUPPORTED_VERSION;
    let fixtures = include_str!("fixtures/doc-examples.tsv");
    for l in fixtures.lines().filter(|l| l.starts_with("file.") && !l.starts_with("file.sandbox")) {
        let (kind, json) = l.split_once('\t').unwrap();
        let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
        v["v"] = serde_json::json!(2);
        let bad = serde_json::to_string(&v).unwrap();
        let e = match kind {
            "file.lkg" => files::decode::<LkgFile>(bad.as_bytes()).unwrap_err(),
            "file.boot" => files::decode::<BootFile>(bad.as_bytes()).unwrap_err(),
            "file.update_failed" => files::decode::<UpdateFailedFile>(bad.as_bytes()).unwrap_err(),
            "file.supervisor" => files::decode::<SupervisorFile>(bad.as_bytes()).unwrap_err(),
            "file.ready" => files::decode::<ReadyFile>(bad.as_bytes()).unwrap_err(),
            "file.heartbeat" => files::decode::<HeartbeatFile>(bad.as_bytes()).unwrap_err(),
            "file.lineage_local" => files::decode::<LineageLocalRecord>(bad.as_bytes()).unwrap_err(),
            _ => unreachable!(),
        };
        assert!(e.to_string().contains(UNSUPPORTED_VERSION), "{kind}: {e}");
    }
    assert!(stdio::decode_to_body(br#"{"t":"interrupt","v":2}"#).is_err());
    assert!(matches!(LinkFrame::parse_line(br#"{"data":{},"ev":"mode","v":2}"#), Err(init_proto::FrameError::Version(_))));
}
