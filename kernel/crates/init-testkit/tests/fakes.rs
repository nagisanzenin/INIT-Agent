//! WP3: the fakes round-trip against each other (init-fakeroot ⇄ init-fakekernel ⇄ init-fakebody)
//! and against the `init-proto` golden examples (kernel/crates/init-proto/tests/fixtures/doc-examples.tsv).
//!
//! Live-safe (testing.md §3.1a): every process here is spawned by the test or by the fake Root's
//! broker and is stopped by its held pid or process group. Homes live under
//! `kernel/target/test-homes/wp3/` and are removed by exact path on success.

use init_testkit::fakeroot::OwnerClient;
use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::{Mock, MockConfig, MockRequest, Mode as MockMode, Script, client};
use init_testkit::util;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

const FAKEROOT: &str = env!("CARGO_BIN_EXE_init-fakeroot");
const FAKEKERNEL: &str = env!("CARGO_BIN_EXE_init-fakekernel");
const FAKEBODY: &str = env!("CARGO_BIN_EXE_init-fakebody");

fn golden(kind: &str) -> Vec<Value> {
    let p =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../init-proto/tests/fixtures/doc-examples.tsv");
    fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(k, _)| *k == kind)
        .map(|(_, j)| serde_json::from_str(j).unwrap())
        .collect()
}

/// Every key of `g` (recursively, for objects) is present in `a` with the same JSON type; a null
/// in `g` accepts anything. Arrays compare their first elements when both have one.
fn same_shape(g: &Value, a: &Value, path: &str) {
    match (g, a) {
        (Value::Null, _) => {}
        (Value::Object(go), Value::Object(ao)) => {
            for (k, gv) in go {
                let av = ao
                    .get(k)
                    .unwrap_or_else(|| panic!("{path}.{k}: missing in {a}"));
                same_shape(gv, av, &format!("{path}.{k}"));
            }
        }
        (Value::Array(ga), Value::Array(aa)) => {
            if let (Some(x), Some(y)) = (ga.first(), aa.first()) {
                same_shape(x, y, &format!("{path}[0]"));
            }
        }
        (Value::String(_), Value::String(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::Bool(_), Value::Bool(_)) => {}
        (Value::String(_), Value::Null) | (Value::Number(_), Value::Null) => {} // optional fields the fake leaves null
        _ => panic!("{path}: golden {g} vs actual {a}"),
    }
}

struct Stack {
    dir: PathBuf,
    home: PathBuf,
    rh: PathBuf,
    root: Child,
    _mock: Option<MockServer>,
}

impl Drop for Stack {
    /// On a failed test: stop the fake Root (held child) in order; its kernel and bodies follow.
    fn drop(&mut self) {
        if self.root.try_wait().ok().flatten().is_none() {
            let _ = init_testkit::sys::kill(self.root.id() as i32, init_testkit::sys::SIGTERM);
            for _ in 0..200 {
                if self.root.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = self.root.kill();
            let _ = self.root.wait();
        }
    }
}

fn charter(dir: &Path, extra: &str) -> PathBuf {
    let p = dir.join("charter.toml");
    fs::write(
        &p,
        format!(
            "schema_version = 1\n[autonomy]\nmax_level = 4\n[money]\nday_usd = \"5.00\"\ntotal_usd = \"25.00\"\n\
             [quotas]\nmax_instances = 6\nparallel_bodies = 2\n[root]\ntest_mode = true\nhb_interval_s = 1\n\
             [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\nreserve_output_tokens = 1000\n\
             [actions.providers.models.\"deepseek-v4-pro\"]\nmin_level = 1\ninput_musd_per_mtok = 1320000\noutput_musd_per_mtok = 3960000\n{extra}"
        ),
    )
    .unwrap();
    p
}

fn start(
    name: &str,
    kmode: &str,
    root_mode: &str,
    extra_charter: &str,
    mock: Option<Script>,
) -> Stack {
    start_env(name, kmode, root_mode, extra_charter, mock, &[])
}

fn start_env(
    name: &str,
    kmode: &str,
    root_mode: &str,
    extra_charter: &str,
    mock: Option<Script>,
    env: &[(&str, &str)],
) -> Stack {
    let dir = util::test_dir(name);
    let home = dir.join("h");
    let rh = dir.join("r");
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::copy(FAKEBODY, home.join("bin/init-body")).unwrap();
    let ch = charter(&dir, extra_charter);
    let mock = mock.map(|s| {
        let mut cfg = MockConfig::new(MockMode::Upstream, s);
        cfg.strict_deepseek = true;
        cfg.record_dir = Some(dir.join("m"));
        MockServer::start(Mock::new(cfg).unwrap(), Listen::Tcp("127.0.0.1:0".into())).unwrap()
    });
    let mut cmd = Command::new(FAKEROOT);
    cmd.args([
        "--root-home",
        rh.to_str().unwrap(),
        "--home",
        home.to_str().unwrap(),
        "--kernel",
        FAKEKERNEL,
        "--charter",
        ch.to_str().unwrap(),
    ])
    .args(["--mode", root_mode])
    .env("FAKEKERNEL_MODE", kmode)
    .env("INIT_TEST_FAKEBODY_HB_MS", "200")
    .env("INIT_TEST_FAKEKERNEL_BACKOFF_MS", "20")
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .envs(env.iter().copied())
    .process_group(0);
    if let Some(m) = &mock {
        cmd.args([
            "--upstream",
            &format!("http://127.0.0.1:{}", m.port().unwrap()),
        ]);
    }
    use std::os::unix::process::CommandExt;
    let root = init_testkit::watchdog::spawn(&mut cmd).unwrap();
    let sock = rh.join("run/owner.sock");
    util::poll(Duration::from_secs(10), Duration::from_millis(20), || {
        sock.exists().then_some(())
    })
    .expect("owner.sock");
    Stack {
        dir,
        home,
        rh,
        root,
        _mock: mock,
    }
}

impl Stack {
    fn owner(&self) -> OwnerClient {
        OwnerClient::connect(&self.rh.join("run/owner.sock")).unwrap()
    }

    fn status(&self) -> Value {
        self.owner().request("status", json!({})).unwrap()["result"].clone()
    }

    fn wait_status(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        util::poll(Duration::from_secs(20), Duration::from_millis(50), || {
            let s = self.status();
            f(&s).then_some(s)
        })
        .unwrap_or_else(|| panic!("timed out waiting for {what}: {}", self.status()))
    }

    fn events(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let mut files: Vec<_> = fs::read_dir(self.home.join("state/sessions"))
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            out.extend(util::read_jsonl(&f));
        }
        out
    }

    fn lineage(&self) -> Vec<Value> {
        util::read_jsonl(&self.rh.join("lineage/lineage.jsonl"))
    }

    fn kernel_log(&self) -> Vec<Value> {
        util::read_jsonl(&self.home.join("state/kernel/fakekernel.jsonl"))
    }

    fn root_log(&self) -> Vec<Value> {
        util::read_jsonl(&self.rh.join("logs/fakeroot.jsonl"))
    }

    fn finish(mut self) {
        let _ = self.owner().request("shutdown", json!({}));
        let t = Instant::now();
        let code = loop {
            if let Some(s) = self.root.try_wait().unwrap() {
                break s.code();
            }
            if t.elapsed() > Duration::from_secs(20) {
                let _ = self.root.kill();
                panic!("fakeroot did not shut down");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(code, Some(0));
        let st: Value =
            serde_json::from_slice(&fs::read(self.rh.join("state/root.json")).unwrap()).unwrap();
        assert_eq!(
            st["instances"],
            json!([]),
            "no instance survives the fake Root"
        );
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

/// Read owner frames until one matches `f`.
fn read_until(c: &mut OwnerClient, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    c.set_timeout(Duration::from_secs(20)).unwrap();
    loop {
        let v = c
            .read()
            .unwrap_or_else(|e| panic!("waiting for {what}: {e}"));
        if f(&v) {
            return v;
        }
    }
}

#[test]
fn full_stack_round_trip() {
    let _wd = init_testkit::watchdog::arm("full_stack_round_trip", WD);
    let s = start("rt", "ok", "ok", "", None);
    let st = s.wait_status("kernel ready and one body", |st| {
        st["kernel"]["ready"] == true && st["bodies"] == 1
    });
    assert_eq!(st["slot"], "running");
    let ready = s.home.join("state/run/ready");
    util::poll(Duration::from_secs(10), Duration::from_millis(20), || {
        ready.exists().then_some(())
    })
    .expect("body ready");

    // Owner line → owner.in → inbox → body input → ack + display → owner.out → attached client.
    let mut c = s.owner();
    c.frame(&json!({"t": "attach", "replay": false})).unwrap();
    read_until(&mut c, "replay_end", |v| v["t"] == "replay_end");
    c.frame(&json!({"t": "line", "text": "hello fakes"}))
        .unwrap();
    let echo = read_until(&mut c, "root echo", |v| {
        v["t"] == "echo" && v.get("qid").is_some()
    });
    assert_eq!(echo["text"], "hello fakes");
    let d = read_until(&mut c, "the body's reply", |v| {
        v["t"] == "display" && v["kind"] == "text"
    });
    assert_eq!(d["text"], "fake reply: hello fakes");
    assert!(d["boot_seq"].as_u64().unwrap() >= 1);

    // The session log holds boot, the owner message (with input_id) and the reply.
    let ev = s.events();
    let boot = ev.iter().find(|e| e["type"] == "boot").expect("boot event");
    assert_eq!(boot["reason"], "cold");
    let user = ev
        .iter()
        .find(|e| e["type"] == "message" && e["role"] == "user")
        .expect("user message");
    assert_eq!(user["content"], "hello fakes");
    assert_eq!(user["input_id"].as_str().unwrap().len(), 16);
    assert!(
        ev.iter()
            .any(|e| e["role"] == "assistant" && e["content"] == "fake reply: hello fakes")
    );
    // The kernel's durable inbox has the line with the Root's qid.
    let inbox = util::read_jsonl(&s.home.join("state/kernel/inbox.jsonl"));
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0]["qid"], echo["qid"]);
    // The owner queue is empty once the kernel acknowledged.
    s.wait_status("queue drained", |st| st["queued"] == 0);

    // Lineage: the kernel's scripted LKG report (store.put + promotion) is acknowledged and chained.
    let lin = s.lineage();
    let promo = lin
        .iter()
        .find(|r| r["kind"] == "promotion")
        .expect("promotion record");
    assert!(promo["by"].as_str().unwrap().starts_with("kernel:"));
    assert_eq!(promo["data"]["lkg"], true);
    let blob = s.rh.join(format!(
        "store/blobs/{}",
        promo["data"]["body_binary"].as_str().unwrap()
    ));
    assert!(blob.exists(), "store.put round trip");
    for w in lin.windows(2) {
        assert_eq!(w[1]["prev"], w[0]["hash"]);
        assert_eq!(
            w[1]["seq"].as_u64().unwrap(),
            w[0]["seq"].as_u64().unwrap() + 1
        );
    }
    assert!(lin.iter().any(|r| r["kind"] == "zone_start"));
    let klog = s.kernel_log();
    let ack = klog
        .iter()
        .find(|r| r["ev"] == "request" && r["cmd"] == "lineage.append")
        .unwrap();
    assert_eq!(ack["resp"]["result"]["dup"], false);
    assert!(
        ack["resp"]["result"]["hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    // Heartbeats reach the Root.
    s.wait_status("hb", |st| st["hb"]["count"].as_u64().unwrap_or(0) >= 1);

    // A level event is relayed to the body as a notice and acknowledged into the log.
    let r = s.owner().request("test.level", json!({"to": 1})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    util::poll(Duration::from_secs(10), Duration::from_millis(50), || {
        s.events()
            .iter()
            .any(|e| e["type"] == "notice" && e["kind"] == "level")
            .then_some(())
    })
    .expect("level notice in the log");

    // Owner `stop` → Root `stop` → kernel stops the body → kernel exit 0.
    let r = s
        .owner()
        .request("stop", json!({"grace_ms": 5000}))
        .unwrap();
    assert_eq!(r["result"]["exit"]["code"], 0, "{r}");
    assert_eq!(r["result"]["answer"]["ok"], true, "{r}");
    let st = s.wait_status("no instances", |st| st["instances"] == json!([]));
    assert_eq!(st["slot"], "stopped");
    s.finish();
}

#[test]
fn frames_match_init_proto_golden_examples() {
    let _wd = init_testkit::watchdog::arm("frames_match_init_proto_golden_examples", WD);
    let s = start("gold", "ok", "ok", "", None);
    s.wait_status("kernel ready and one body", |st| {
        st["kernel"]["ready"] == true && st["bodies"] == 1
    });
    s.wait_status("hb", |st| st["hb"]["count"].as_u64().unwrap_or(0) >= 1);
    // The Root records the instance before it answers; wait until the kernel has seen the body ready.
    let klog = util::poll(Duration::from_secs(10), Duration::from_millis(20), || {
        let l = s.kernel_log();
        l.iter().any(|r| r["ev"] == "body_ready").then_some(l)
    })
    .expect("body_ready in the kernel log");
    // Frames the fake kernel received from the fake Root.
    let inbound: Vec<&Value> = klog
        .iter()
        .filter(|r| r["link"] == true && r["dir"] == "in")
        .map(|r| &r["frame"])
        .collect();
    let resp = |id: &str| {
        inbound
            .iter()
            .find(|f| f["id"] == id && f.get("ok").is_some())
            .copied()
            .unwrap_or_else(|| panic!("no response {id}"))
    };
    // hello is k-1: the boot block has the shape of root.md §4.1.3's example and types as BootBlock.
    let bb = &resp("k-1")["result"];
    same_shape(&golden("link.boot_block")[0], bb, "boot_block");
    let _: init_proto::root_link::BootBlock = serde_json::from_value(bb.clone()).unwrap();
    // spawn response and fd tag.
    let spawn_req = klog
        .iter()
        .find(|r| r["dir"] == "out" && r["frame"]["cmd"] == "spawn")
        .unwrap();
    let sid = spawn_req["frame"]["id"].as_str().unwrap();
    same_shape(&golden("link.response.spawn")[0], resp(sid), "spawn");
    // ledger.get / ceilings.get (asked by the kernel on start).
    for (cmd, g) in [
        ("ledger.get", "link.ledger"),
        ("ceilings.get", "link.ceilings"),
    ] {
        let id = klog
            .iter()
            .find(|r| r["dir"] == "out" && r["frame"]["cmd"] == cmd)
            .unwrap()["frame"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        same_shape(&golden(g)[0], &resp(&id)["result"], cmd);
    }
    // Every line either fake wrote on the link is canonical ICJ and parses as an init-proto frame.
    let rlog = s.root_log();
    let all: Vec<&Value> = klog
        .iter()
        .chain(rlog.iter())
        .filter(|r| r["link"] == true)
        .map(|r| &r["frame"])
        .collect();
    assert!(all.len() > 10);
    for f in all {
        let line = serde_json::to_vec(f).unwrap();
        assert!(init_proto::icj::is_canonical(&line), "not ICJ: {f}");
        init_proto::envelope::LinkFrame::parse_line(&line).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
    // Stop the body so an `exited` event flows, and compare it with the golden one.
    let r = s
        .owner()
        .request("stop", json!({"grace_ms": 5000}))
        .unwrap();
    assert_eq!(r["result"]["exit"]["code"], 0);
    let klog = s.kernel_log();
    let exited = klog
        .iter()
        .find(|r| r["dir"] == "in" && r["frame"]["ev"] == "exited")
        .expect("exited event");
    same_shape(&golden("link.event.root")[0], &exited["frame"], "exited");
    s.finish();
}

/// The fake body reproduces kernel/supervise.md §4.5's example exchange (golden `stdio.*` rows):
/// input → state busy, ack{18}, display…; pause → paused{token}; exit{75} → exit code 75.
#[test]
fn fakebody_speaks_the_golden_stdio_exchange() {
    let _wd = init_testkit::watchdog::arm("fakebody_speaks_the_golden_stdio_exchange", WD);
    let dir = util::test_dir("stdio");
    let to_body = golden("stdio.to_body");
    let mut child = init_testkit::watchdog::spawn(Command::new(FAKEBODY)
        .env_clear()
        .env("INIT_HOME", &dir)
        .env("INIT_UI", "stdio-jsonl/1")
        .env("INIT_BOOT_ID", "a3f09c1e5b7d2468")
        .env("INIT_SESSION", "20261019-120000-3fa9c1")
        .env("INIT_SHA", "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()))
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let ready = dir.join("state/run/ready");
    util::poll(Duration::from_secs(10), Duration::from_millis(10), || {
        ready.exists().then_some(())
    })
    .unwrap();
    let r: init_proto::files::ReadyFile =
        init_proto::files::decode(&fs::read(&ready).unwrap()).unwrap();
    same_shape(
        &golden("file.ready")[0],
        &serde_json::to_value(&r).unwrap(),
        "ready",
    );
    assert_eq!(r.boot_id, "a3f09c1e5b7d2468");
    let mut read = || {
        let mut l = String::new();
        out.read_line(&mut l).unwrap();
        init_proto::stdio::decode_from_body(l.as_bytes()).unwrap_or_else(|e| panic!("{l}: {e}"))
    };
    use init_proto::stdio::FromBody;
    // 1. input (the golden frame, byte for byte).
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&to_body[0]).unwrap()).as_bytes())
        .unwrap();
    assert_eq!(read(), FromBody::State { busy: true });
    assert_eq!(read(), FromBody::Ack { seq: 18 });
    assert!(matches!(read(), FromBody::Display(d) if d.text == "fake reply: add a pty tool"));
    assert!(matches!(read(), FromBody::Display(_))); // turn_end
    assert_eq!(read(), FromBody::State { busy: false });
    // Redelivery of the same input id is acknowledged without a second log entry.
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&to_body[0]).unwrap()).as_bytes())
        .unwrap();
    assert_eq!(read(), FromBody::Ack { seq: 18 });
    // 2. pause.
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&to_body[1]).unwrap()).as_bytes())
        .unwrap();
    let FromBody::Paused {
        token,
        log_len,
        leaf,
    } = read()
    else {
        panic!("expected paused")
    };
    assert_eq!(token, "5f0d22aa91c3e7b4");
    assert!(log_len > 0 && leaf.len() == 16);
    // 3. exit{75}.
    stdin
        .write_all(format!("{}\n", serde_json::to_string(&to_body[2]).unwrap()).as_bytes())
        .unwrap();
    let st = child.wait().unwrap();
    assert_eq!(st.code(), Some(75));
    let log = util::read_jsonl(&dir.join("state/sessions/20261019-120000-3fa9c1.jsonl"));
    assert_eq!(
        log.iter()
            .filter(|e| e["type"] == "message" && e["role"] == "user")
            .count(),
        1,
        "dedup by input_id"
    );
    let hb: init_proto::files::HeartbeatFile =
        init_proto::files::decode(&fs::read(dir.join("state/run/heartbeat")).unwrap()).unwrap();
    assert_eq!(hb.boot_id, "a3f09c1e5b7d2468");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn egress_forwards_to_the_mock_and_meters_at_peak_price() {
    let _wd = init_testkit::watchdog::arm("egress_forwards_to_the_mock_and_meters_at_peak_price", WD);
    let script = Script::parse(
        "{\"v\":1,\"kind\":\"meta\",\"provider\":\"deepseek\",\"model\":\"deepseek-flash\"}\n{\"v\":1,\"kind\":\"turn\",\"reply\":{\"text\":\"one\"}}\n{\"v\":1,\"kind\":\"turn\",\"reply\":{\"text\":\"two\"}}\n",
        Path::new("."),
    )
    .unwrap();
    let s = start("egr", "bypass_soft:2,no_body", "ok", "", Some(script));
    s.wait_status("kernel ready", |st| st["kernel"]["ready"] == true);
    let klog = util::poll(Duration::from_secs(15), Duration::from_millis(50), || {
        let l = s.kernel_log();
        (l.iter().filter(|r| r["ev"] == "egress").count() == 2).then_some(l)
    })
    .expect("two bypass requests");
    for r in klog.iter().filter(|r| r["ev"] == "egress") {
        assert_eq!(r["status"], 200, "{r}");
    }
    // Spend is settled at the charter's peak prices; the requests carried no guard header.
    let st = s.status();
    assert!(st["money"]["day_spent_musd"].as_u64().unwrap() > 0, "{st}");
    assert_eq!(st["money"]["requests"], 2);
    let usage: Vec<Value> = fs::read_dir(s.rh.join("egress/usage"))
        .unwrap()
        .flatten()
        .flat_map(|f| util::read_jsonl(&f.path()))
        .collect();
    assert!(
        usage
            .iter()
            .filter(|u| u["ev"] == "start")
            .all(|u| u["bypass"] == true)
    );
    assert!(
        usage
            .iter()
            .filter(|u| u["ev"] == "end")
            .all(|u| u["usage"]["output"].as_u64().is_some())
    );
    // The mock saw strict-valid requests with no x-init-* headers (upstream mode).
    let mlog = init_testkit::mockllm::read_log(&s.dir.join("m/mock.log.jsonl")).unwrap();
    assert_eq!(mlog.len(), 2);
    assert!(
        mlog.iter()
            .all(|l| l.protocol_error.is_none() && l.status == 200),
        "{mlog:?}"
    );

    // Direct requests on egress.sock: ceilings, a refused model, a level-gated model, no tunnel.
    let sock = s.rh.join("run/egress.sock");
    let c = client::request_unix(&sock, &MockRequest::get("/_root/ceilings")).unwrap();
    let cv: Value = serde_json::from_slice(&c.body).unwrap();
    same_shape(&golden("link.ceilings")[0], &cv, "GET /_root/ceilings");
    assert_eq!(cv["money"]["day_limit_musd"], 5_000_000);
    let post = |model: &str| {
        let b =
            json!({"model": model, "messages": [{"role": "user", "content": "x"}], "stream": true});
        client::request_unix(
            &sock,
            &MockRequest::post(
                "/deepseek/chat/completions",
                &[("content-type", "application/json")],
                b.to_string(),
            ),
        )
        .unwrap()
    };
    let r = post("gpt-unknown");
    assert_eq!(
        (r.status, r.header("x-init-egress-error")),
        (403, Some("class.model"))
    );
    let r = post("deepseek-v4-pro");
    assert_eq!(
        (r.status, r.header("x-init-egress-error")),
        (403, Some("class.level"))
    );
    let body: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["init"]["source"], "root");
    let r = client::request_unix(
        &sock,
        &MockRequest {
            method: "CONNECT".into(),
            path: "example.com:443".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.header("x-init-egress-error"), Some("class.destination"));
    let r = client::request_unix(
        &sock,
        &MockRequest::get("/x/crates/index.crates.io/se/rd/serde"),
    )
    .unwrap();
    assert_eq!(r.header("x-init-egress-error"), Some("class.disabled"));
    s.finish();
}

#[test]
fn tiny_ceiling_refuses_with_typed_429() {
    let _wd = init_testkit::watchdog::arm("tiny_ceiling_refuses_with_typed_429", WD);
    // A day ceiling below one reservation: the first metered request is refused `ceiling.day`.
    let s = start("ceil", "no_body", "ok", "", None);
    s.wait_status("kernel ready", |st| st["kernel"]["ready"] == true);
    s.finish();
    let dir = util::test_dir("ceil2");
    let ch = dir.join("tiny.toml");
    fs::write(&ch, "[money]\nday_usd = \"0.000100\"\ntotal_usd = \"1.00\"\n[actions.providers.models.\"deepseek-flash\"]\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n").unwrap();
    let home = dir.join("h");
    let rh = dir.join("r");
    let mut root = init_testkit::watchdog::spawn(Command::new(FAKEROOT)
        .args([
            "--root-home",
            rh.to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
            "--kernel",
            FAKEKERNEL,
            "--charter",
            ch.to_str().unwrap(),
            "--no-start",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap();
    let sock = rh.join("run/egress.sock");
    util::poll(Duration::from_secs(10), Duration::from_millis(20), || {
        sock.exists().then_some(())
    })
    .unwrap();
    let b = json!({"model": "deepseek-flash", "messages": [], "max_tokens": 1000});
    let r = client::request_unix(
        &sock,
        &MockRequest::post(
            "/deepseek/chat/completions",
            &[("x-egress-guard", "epoch=0")],
            b.to_string(),
        ),
    )
    .unwrap();
    assert_eq!(r.status, 429);
    assert_eq!(r.header("x-init-egress-error"), Some("ceiling.day"));
    assert!(r.header("retry-after").is_some());
    let v: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(v["init"]["limit_musd"], 100);
    assert_eq!(v["init"]["demotes"], false, "the guard header was present");
    assert_eq!(v["error"]["type"], "init_ceiling_exceeded");
    let mut c = OwnerClient::connect(&rh.join("run/owner.sock")).unwrap();
    assert_eq!(c.request("shutdown", json!({})).unwrap()["ok"], true);
    assert_eq!(root.wait().unwrap().code(), Some(0));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn panic_from_socket_and_latch_then_resume() {
    let _wd = init_testkit::watchdog::arm("panic_from_socket_and_latch_then_resume", WD);
    let s = start("pan", "ok", "level:2", "", None);
    s.wait_status("ready with a body", |st| {
        st["kernel"]["ready"] == true && st["bodies"] == 1
    });
    let r = s.owner().request("panic", json!({})).unwrap();
    assert_eq!(r["result"]["channel"], "owner_socket");
    let st = s.wait_status("everything stopped", |st| {
        st["kernel"].is_null() && st["instances"] == json!([])
    });
    assert_eq!(st["slot"], "panicked");
    assert_eq!(st["level"], 1, "panic demotes one level (CH-2)");
    assert!(s.rh.join("run/panic").exists());
    let lin = s.lineage();
    assert!(
        lin.iter()
            .any(|r| r["kind"] == "panic" && r["data"]["channel"] == "owner_socket")
    );
    assert!(
        lin.iter()
            .any(|r| r["kind"] == "ladder" && r["data"]["trigger"]["kind"] == "panic")
    );
    // The egress is closed while panicked.
    let r = client::request_unix(
        &s.rh.join("run/egress.sock"),
        &MockRequest::post("/deepseek/chat/completions", &[], "{}"),
    )
    .unwrap();
    assert_eq!(r.status, 503);
    // Resume deletes the latch and starts the kernel again.
    let r = s.owner().request("resume", json!({})).unwrap();
    assert_eq!(r["result"]["started"], true, "{r}");
    s.wait_status("running again", |st| {
        st["kernel"]["ready"] == true
            && st["bodies"] == 1
            && st["kernel"]["reason"] == "panic_resume"
    });
    assert!(!s.rh.join("run/panic").exists());
    // The latch-file channel.
    fs::write(s.rh.join("run/panic"), "{}").unwrap();
    let st = s.wait_status("latch panic", |st| {
        st["slot"] == "panicked" && st["kernel"].is_null()
    });
    assert_eq!(st["panicked"]["channel"], "latch");
    s.finish();
}

#[test]
fn prepare_swap_exits_75_and_the_kernel_restarts() {
    let _wd = init_testkit::watchdog::arm("prepare_swap_exits_75_and_the_kernel_restarts", WD);
    let s = start("swap", "ok", "ok", "", None);
    s.wait_status("ready with a body", |st| {
        st["kernel"]["ready"] == true && st["bodies"] == 1
    });
    let r = s
        .owner()
        .request(
            "test.prepare_swap",
            json!({"job": "s-000001", "deadline_s": 10}),
        )
        .unwrap();
    assert_eq!(r["result"]["exit"]["code"], 75, "{r}");
    assert_eq!(r["result"]["restarted"], true);
    s.wait_status("upgraded kernel running", |st| {
        st["kernel"]["ready"] == true
            && st["kernel"]["reason"] == "kernel_upgrade"
            && st["bodies"] == 1
    });
    // The body was paused (reason swap) and stopped, not killed.
    let lin = s.lineage();
    assert!(
        lin.iter()
            .any(|r| r["kind"] == "zone_exit" && r["data"]["class"] == "swap")
    );
    s.finish();
}

#[test]
fn hostile_kernel_extra_bodies_and_level_gated_requests() {
    let _wd = init_testkit::watchdog::arm("hostile_kernel_extra_bodies_and_level_gated_requests", WD);
    let s = start(
        "host",
        "extra_bodies:1,ignore_level,egress:example.com",
        "ok",
        "",
        None,
    );
    s.wait_status("ready with a body", |st| {
        st["kernel"]["ready"] == true && st["bodies"] == 1
    });
    let klog = util::poll(Duration::from_secs(10), Duration::from_millis(50), || {
        let l = s.kernel_log();
        l.iter()
            .any(|r| r["ev"] == "egress" && r["path"] == "CONNECT")
            .then_some(l)
    })
    .unwrap();
    let spawns: Vec<&Value> = klog
        .iter()
        .filter(|r| r["ev"] == "request" && r["cmd"] == "spawn")
        .collect();
    assert_eq!(spawns.len(), 2);
    assert_eq!(
        spawns[1]["resp"]["error"]["code"], "root.level",
        "a second body at L0 is level-gated"
    );
    let code = |cmd: &str| {
        klog.iter()
            .find(|r| r["ev"] == "request" && r["cmd"] == cmd)
            .unwrap()["resp"]["error"]["code"]
            .clone()
    };
    assert_eq!(code("system.swap"), "root.level");
    assert_eq!(code("sealed.family_eval"), "root.level");
    assert!(s.lineage().iter().any(|r| r["kind"] == "ceiling_hit"
        && r["data"]["demotes"] == true
        && r["data"]["source"] == "broker"));
    let codes: Vec<&Value> = klog
        .iter()
        .filter(|r| r["ev"] == "egress")
        .map(|r| &r["error_code"])
        .collect();
    assert_eq!(
        codes,
        vec![
            &json!("class.disabled"),
            &json!("class.unknown"),
            &json!("class.destination")
        ]
    );
    s.finish();
}

#[test]
fn crash_first_body_is_restarted_by_the_kernel() {
    let _wd = init_testkit::watchdog::arm("crash_first_body_is_restarted_by_the_kernel", WD);
    // The fake body crashes right after ready on its first two boots, then runs (A5-fake shape).
    let s = start_env(
        "crash",
        "ok",
        "ok",
        "",
        None,
        &[("FAKEBODY_MODE", "crash_first:2")],
    );
    let st = s.wait_status("a stable body after two crashes", |st| {
        st["kernel"]["ready"] == true
            && st["bodies"] == 1
            && fs::read_to_string(s.home.join("state/fakebody.count"))
                .is_ok_and(|c| c.trim() == "3")
    });
    assert_eq!(st["slot"], "running");
    let klog = s.kernel_log();
    let exits: Vec<&Value> = klog.iter().filter(|r| r["ev"] == "body_exit").collect();
    assert_eq!(exits.len(), 2, "{exits:?}");
    assert!(exits.iter().all(|e| e["code"] == 101));
    let boots: Vec<Value> = s
        .events()
        .into_iter()
        .filter(|e| e["type"] == "boot")
        .collect();
    assert_eq!(
        boots
            .iter()
            .map(|b| b["reason"].as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        vec!["cold", "crash", "crash"]
    );
    s.finish();
}
