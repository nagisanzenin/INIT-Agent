//! UX-KEYMODEL tests ($0, no network): the catalog, the charter and kernel fragments against
//! docs/provider-configuration.md's own examples, the section-level edits, `model use`, and the key path (stored
//! 0600, never in a log, test mode refuses a non-`test-` key).

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const DOC: &str = include_str!("../../../../../docs/provider-configuration.md");

/// The fenced ```toml blocks of the provider doc, in order.
fn doc_blocks() -> Vec<Table> {
    let mut out = Vec::new();
    let mut it = DOC.split("```toml\n").skip(1);
    for b in it.by_ref() {
        let body = b.split("```").next().unwrap();
        out.push(toml::from_str::<Table>(body).unwrap_or_else(|e| panic!("doc block does not parse: {e}\n{body}")));
    }
    out
}

fn at<'a>(t: &'a Table, path: &[&str]) -> &'a toml::Value {
    get(t, path).unwrap_or_else(|| panic!("missing {path:?} in {t:#?}"))
}

fn req(preset: &str, url: Option<&str>, model: Option<&str>, name: Option<&str>, price: Option<&str>) -> AddReq {
    AddReq { preset: preset.into(), base_url: url.map(Into::into), model: model.map(Into::into), name: name.map(Into::into),
        price: price.map(Into::into), ..AddReq::default() }
}

fn home(tag: &str) -> (PathBuf, Paths) {
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes");
    fs::create_dir_all(&parent).unwrap();
    let dir = fs::canonicalize(parent).unwrap().join(format!("uxk{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir); // exact path, created by this test
    let p = Paths { rh: dir.join("rh"), ih: dir.join("ih"), home: dir.clone() };
    fs::create_dir_all(&p.rh).unwrap();
    (dir, p)
}

fn template() -> String {
    charter::template().replace("REPLACE ME: what INIT is for, in your words.", "UX-KEYMODEL test.")
        .replace("disk_min_free_gib = 20", "disk_min_free_gib = 0") + "\n[root]\ntest_mode = true\n"
}

fn installed(tag: &str, charter_src: &str) -> (PathBuf, std::sync::Arc<Root>) {
    let (dir, p) = home(tag);
    fs::write(p.rh.join("charter.toml"), charter_src).unwrap();
    let o = crate::install::Opts { release: None, charter: None, git: "/usr/bin/git".into(), with_testkit: false, toolchain: None,
        seed_source: None, seed_tag: None, no_service: true, bin_dir: None };
    crate::install::install(&p, &o).unwrap_or_else(|e| panic!("install: {e:?}"));
    (dir, Root::boot(p).unwrap())
}

#[test]
fn catalog_only_deepseek_is_live_and_prices_are_exact() {
    assert_eq!(CATALOG.iter().map(|p| (p.id, p.tested)).collect::<Vec<_>>(),
        vec![("deepseek", "live"), ("gateway", "experimental"), ("anthropic", "experimental"), ("openai", "experimental"), ("local", "experimental")]);
    let ds = preset("deepseek").unwrap();
    assert_eq!(ds.models.iter().map(|m| m.id).collect::<Vec<_>>(), ["deepseek-flash", "deepseek-v4-pro"]);
    // the charter template's built-in deepseek-flash tariff (µUSD/MTok) is the catalog's
    let t: Table = toml::from_str(&charter::template()).unwrap();
    let f = at(&t, &["actions", "providers", "models", "deepseek-flash"]).as_table().unwrap();
    assert_eq!((f["input_musd_per_mtok"].as_integer(), f["cache_read_musd_per_mtok"].as_integer(), f["output_musd_per_mtok"].as_integer()),
        (Some(300_000), Some(6_000), Some(1_200_000)));
    assert_eq!((usd(300_000), usd(6_000), usd(1_200_000), usd(0), usd(1_000_000), usd(1_320_000), usd(44_000)),
        ("0.30".into(), "0.006".into(), "1.20".into(), "0.00".into(), "1.00".into(), "1.32".into(), "0.044".into()));
    assert_eq!(parse_price("1.00,2.00"), Ok((1_000_000, 1_000_000, 2_000_000)));
    assert_eq!(parse_price("1,0.1,2.5"), Ok((1_000_000, 100_000, 2_500_000)));
    assert!(parse_price("1").is_err() && parse_price("a,b").is_err() && parse_price("-1,2").is_err());
}

#[test]
fn urls() {
    assert_eq!(parse_url("https://gateway.example/team/v1", false), Ok(("gateway.example:443".into(), "/team/v1".into())));
    assert_eq!(parse_url("https://OpenRouter.ai:8443/api/v1/", false), Ok(("openrouter.ai:8443".into(), "/api/v1".into())));
    assert_eq!(parse_url("https://gw.example", false), Ok(("gw.example:443".into(), "".into())));
    assert_eq!(parse_url("http://127.0.0.1:11434/v1", true), Ok(("127.0.0.1:11434".into(), "/v1".into())));
    for bad in ["http://gw.example/v1", "https://1.2.3.4/v1", "https://localhost:8080", "https://gw.example/a/../b", "https://u@gw.example",
        "https://gw.example/v1?x=1", "https://gw.example/%2e%2e", "ftp://gw.example"] {
        assert!(parse_url(bad, false).is_err(), "{bad}");
    }
    for bad in ["http://192.168.1.2:11434/v1", "http://127.0.0.1/v1", "https://127.0.0.1:1234"] {
        assert!(parse_url(bad, true).is_err(), "{bad}");
    }
}

#[test]
fn specs_refuse_what_cannot_work() {
    assert!(spec(&req("nope", None, None, None, None)).is_err());
    assert!(spec(&req("gateway", None, Some("m"), None, None)).is_err(), "a gateway needs a URL");
    assert!(spec(&req("gateway", Some("https://gw.example/v1"), None, None, None)).is_err(), "and a model");
    assert!(spec(&req("deepseek", None, Some("deepseek-r9"), None, None)).is_err(), "not in the catalog");
    assert!(spec(&req("deepseek", Some("https://gw.example"), None, None, None)).is_err(), "deepseek: the official host only");
    assert!(spec(&req("gateway", Some("https://gw.example/v1"), Some("m"), Some("deepseek"), None)).is_err(), "an official name");
    assert!(spec(&req("gateway", Some("https://gw.example/v1"), Some("m"), Some("Bad Name"), None)).is_err());
    let d = spec(&req("deepseek", None, None, None, None)).unwrap();
    assert_eq!((d.alias.as_str(), d.model.as_str(), d.custom, d.tested), ("deepseek", "deepseek-flash", false, "live"));
    let a = spec(&req("anthropic", None, Some("claude-sonnet-5-5"), None, None)).unwrap();
    assert_eq!((a.alias.as_str(), a.host.as_str(), a.custom, a.price, a.tested), ("anthropic", "api.anthropic.com:443", false, (2_000_000, 100_000, 10_000_000), "experimental"));
    let g = spec(&req("anthropic", Some("https://gw.example/team/v1"), Some("claude-opus-5-5"), None, None)).unwrap();
    assert_eq!((g.alias.as_str(), g.base_path.as_str(), g.custom), ("anthropic-gw", "/team", true), "a custom URL is a custom alias with a descriptor");
    let u = spec(&req("gateway", Some("https://gw.example/v1"), Some("m"), None, None)).unwrap();
    assert!(!u.priced && u.price == (0, 0, 0), "no --price: shown as unpriced");
}

/// The Chat gateway example of docs/provider-configuration.md, both halves, from one `providers add`.
#[test]
fn chat_gateway_fragments_equal_the_doc() {
    let b = doc_blocks();
    let s = spec(&req("gateway", Some("https://gateway.example/team/v1"), Some("custom-model"), Some("custom"), Some("1.00,1.00,2.00"))).unwrap();
    let (pv, md) = charter_fragment(&s);
    assert_eq!(&pv, at(&b[0], &["actions", "providers", "providers", "custom"]).as_table().unwrap());
    assert_eq!(&md, at(&b[0], &["actions", "providers", "models", "custom-model"]).as_table().unwrap(), "min_level, µUSD prices, reserve");
    assert_eq!(s.paths, strs(Some(at(&b[0], &["actions", "providers", "paths"]))));
    assert_eq!(s.host, strs(Some(at(&b[0], &["actions", "providers", "destinations"])))[0]);
    // the kernel half: [model], [model.descriptor], [providers.custom] and its model table
    let k = kernel_add("schema_version = 2\n", &s).unwrap();
    let k = kernel_select(&k, "custom", "custom-model", Some(descriptor_lines(s.api, s.context, s.max_output, s.reserve))).unwrap();
    let kt: Table = toml::from_str(&k).unwrap();
    for path in [&["model"][..], &["providers", "custom"]] {
        assert_eq!(at(&kt, path), at(&b[1], path), "{path:?}\n{k}");
    }
}

#[test]
fn local_and_native_fragments_equal_the_doc() {
    let b = doc_blocks();
    let s = spec(&req("local", Some("http://127.0.0.1:11434/v1"), Some("custom-model"), Some("custom"), None)).unwrap();
    let (pv, md) = charter_fragment(&s);
    assert_eq!(&pv, at(&b[2], &["actions", "providers", "providers", "custom"]).as_table().unwrap());
    assert_eq!(strs(Some(at(&b[2], &["actions", "providers", "destinations"]))), vec![s.host.clone()]);
    assert_eq!(s.paths, strs(Some(at(&b[2], &["actions", "providers", "paths"]))));
    assert_eq!((md["input_musd_per_mtok"].as_integer(), md["output_musd_per_mtok"].as_integer()), (Some(0), Some(0)), "a local server is $0");
    assert!(s.priced, "local: $0 is a known price");
    // the doc's table: Responses and Messages gateways
    let r = spec(&req("openai", Some("https://gw.example/team/v1"), Some("m1"), Some("custom"), Some("1,2"))).unwrap();
    let (rp, _) = charter_fragment(&r);
    assert_eq!((rp["api"].as_str(), rp["base_path"].as_str(), rp["auth"].as_str(), r.paths.clone(), r.kernel_preset),
        (Some("responses"), Some("/team"), Some("bearer"), vec!["v1/responses".to_string()], "openai"));
    let m = spec(&req("anthropic", Some("https://gw.example/v1"), Some("m1"), Some("custom"), Some("1,2"))).unwrap();
    let (mp, _) = charter_fragment(&m);
    assert_eq!((mp["api"].as_str(), mp.get("base_path"), mp["auth"].as_str(), m.paths.clone(), m.kernel_preset),
        (Some("messages"), None, Some("x-api-key"), vec!["v1/messages".to_string()], "anthropic"), "normal /v1 upstream: base_path \"\"");
    assert!(descriptor_lines("responses", 131_072, 32_768, 4096).starts_with("api = \"responses\"\n"));
    assert!(!descriptor_lines("messages", 131_072, 32_768, 4096).contains("chat_stream_usage"));
}

#[test]
fn headers_and_multiline_strings() {
    assert_eq!(header("[actions.providers.models.\"deepseek-flash\"]"), Some(vec!["actions".into(), "providers".into(), "models".into(), "deepseek-flash".into()]));
    assert_eq!(header("  [ model . descriptor ]  # c"), Some(vec!["model".into(), "descriptor".into()]));
    assert_eq!(header("[[x]]"), Some(vec!["x".into()]));
    assert_eq!(header("[\"a\", \"b\"],"), None);
    assert_eq!(header("x = [1]"), None);
    let src = "a = 1\n[s]\nt = '''\n[actions.providers]\nnot a header\n'''\n[actions.providers]\nk = 1\n[after]\nz = 2\n";
    let secs = sections(src);
    assert_eq!(secs.iter().map(|s| s.0.clone().map(|h| h.join("."))).collect::<Vec<_>>(),
        vec![None, Some("s".into()), Some("actions.providers".into()), Some("after".into())]);
    let out = replace_sections(src, |p| starts(p, &["actions", "providers"]), "[actions.providers]\nk = 2\n");
    assert_eq!(out, "a = 1\n[s]\nt = '''\n[actions.providers]\nnot a header\n'''\n[actions.providers]\nk = 2\n\n[after]\nz = 2\n");
}

#[test]
fn charter_add_keeps_deepseek_and_the_rest_of_the_file() {
    let (dir, p) = home("ca");
    let src = template();
    let (ch, ..) = charter::parse(&src, &p.ctx()).unwrap();
    let eff = ch.actions["providers"].as_table().unwrap().clone();
    // DeepSeek flash is built in: nothing to write (an existing user's file is untouched)
    let same = charter_add(&src, &eff, &spec(&req("deepseek", None, None, None, None)).unwrap()).unwrap();
    assert_eq!(same, src);
    let s = spec(&req("gateway", Some("https://gw.example/api/v1"), Some("vendor/model-x"), None, Some("0.50,0.05,1.50"))).unwrap();
    let out = charter_add(&src, &eff, &s).unwrap();
    let (ch2, ..) = charter::parse(&out, &p.ctx()).unwrap_or_else(|f| panic!("{f:?}\n{out}"));
    let c = ch2.actions["providers"].as_table().unwrap();
    assert_eq!(strs(c.get("destinations")), ["api.deepseek.com:443", "gw.example:443"]);
    assert_eq!(at(c, &["providers", "deepseek", "host"]).as_str(), Some("api.deepseek.com:443"), "the preset fallback made explicit");
    assert_eq!(at(c, &["providers", "gateway", "base_path"]).as_str(), Some("/api/v1"));
    assert_eq!(at(c, &["models", "vendor/model-x", "output_musd_per_mtok"]).as_integer(), Some(1_500_000));
    assert!(at(c, &["models", "deepseek-flash"]).is_table(), "the DeepSeek tariff stays");
    assert!(strs(c.get("paths")).iter().any(|x| x == "user/balance") && strs(c.get("paths")).iter().any(|x| x == "chat/completions"));
    // everything outside [actions.providers…] is byte-identical: comments, the constitution, [sandbox]
    for keep in ["# ~/.init-root/charter.toml", "read_home = true       # v0.1.1", "[mirror]", charter::CONSTITUTION, "[root]\ntest_mode = true"] {
        assert!(out.contains(keep), "{keep:?} lost");
    }
    assert_eq!(out.matches("[actions.providers]").count(), 1);
    // re-adding with another URL moves the provider; the old destination goes when nothing else uses it
    let (ch3, ..) = charter::parse(&out, &p.ctx()).unwrap();
    let s2 = spec(&req("gateway", Some("https://gw2.example/v1"), Some("vendor/model-x"), None, Some("0.50,0.05,1.50"))).unwrap();
    let out2 = charter_add(&out, ch3.actions["providers"].as_table().unwrap(), &s2).unwrap();
    let (ch4, ..) = charter::parse(&out2, &p.ctx()).unwrap();
    assert_eq!(strs(ch4.actions["providers"].get("destinations")), ["api.deepseek.com:443", "gw2.example:443"]);
    // an existing official provider keeps its host (here a test mock); only the model and its routes are added
    let mock = src.replace("[actions.providers.models.\"deepseek-flash\"]", "[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:4000\"\n\n[actions.providers.models.\"deepseek-flash\"]")
        .replace("destinations = [\"api.deepseek.com:443\"]", "destinations = [\"127.0.0.1:4000\"]");
    let (chm, ..) = charter::parse(&mock, &p.ctx()).unwrap_or_else(|f| panic!("{f:?}"));
    let outm = charter_add(&mock, chm.actions["providers"].as_table().unwrap(), &spec(&req("deepseek", None, Some("deepseek-v4-pro"), None, None)).unwrap()).unwrap();
    let (chm2, ..) = charter::parse(&outm, &p.ctx()).unwrap();
    assert_eq!(strs(chm2.actions["providers"].get("destinations")), ["127.0.0.1:4000"]);
    assert_eq!(at(chm2.actions["providers"].as_table().unwrap(), &["models", "deepseek-v4-pro", "input_musd_per_mtok"]).as_integer(), Some(1_320_000));
    let _ = fs::remove_dir_all(&dir);
}

const INSTALL_SH_CONFIG: &str = r#"# INIT kernel config (docs/kernel/config.md), written by scripts/install.sh. Edit freely, then restart the Root.
schema_version = 2

[model]
provider = "deepseek"
id = "deepseek-flash"

[budget]                 # soft budgets (config.md §4.3)
day_usd = "5.00"

[sandbox]
toolchain = "/x/tc"
developer_dir = ""

[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  { days = "mon-fri", from = "01:00", to = "04:00", use = "peak" },
]
default_dates_tz = "+08:00"
default_dates    = []

[providers.deepseek.models."deepseek-flash"]
max_output            = 393216
reserve_output_tokens = 65536
reference_window      = "peak"
prices.peak    = { input = "0.30", cache_read = "0.006", output = "1.20" }
prices.offpeak = { input = "0.15", cache_read = "0.003", output = "0.60" }
"#;

#[test]
fn kernel_edits_keep_the_owner_text() {
    // deepseek-v4-pro under the existing schedule: peak and offpeak, the same values install.sh writes
    let s = spec(&req("deepseek", None, Some("deepseek-v4-pro"), None, None)).unwrap();
    let k = kernel_add(INSTALL_SH_CONFIG, &s).unwrap();
    assert!(k.starts_with(INSTALL_SH_CONFIG), "appended only");
    let t: Table = toml::from_str(&k).unwrap();
    assert_eq!(at(&t, &["providers", "deepseek", "models", "deepseek-v4-pro", "prices", "offpeak", "output"]).as_str(), Some("1.98"));
    assert_eq!(kernel_add(&k, &s).unwrap(), k, "idempotent");
    let k2 = kernel_select(&k, "deepseek", "deepseek-v4-pro", None).unwrap();
    for keep in ["[budget]                 # soft budgets", "[sandbox]\ntoolchain = \"/x/tc\"", "# INIT kernel config"] {
        assert!(k2.contains(keep), "{keep:?}");
    }
    assert_eq!(selected(&toml::from_str(&k2).unwrap()), ("deepseek".into(), "deepseek-v4-pro".into()));
    // a custom model gets a descriptor; selecting a built-in model again drops it
    let g = spec(&req("gateway", Some("https://gw.example/v1"), Some("m-x"), None, Some("1,2"))).unwrap();
    let k3 = kernel_select(&kernel_add(&k2, &g).unwrap(), "gateway", "m-x", Some(descriptor_lines("chat", 131_072, 32_768, 4096))).unwrap();
    let t3: Table = toml::from_str(&k3).unwrap();
    assert_eq!(at(&t3, &["model", "descriptor", "api"]).as_str(), Some("chat_completions"));
    assert_eq!(at(&t3, &["providers", "gateway", "preset"]).as_str(), Some("deepseek"));
    let k4 = kernel_select(&k3, "deepseek", "deepseek-flash", None).unwrap();
    assert!(get(&toml::from_str(&k4).unwrap(), &["model", "descriptor"]).is_none());
    // a Root-written template (no [model], no providers): both are added
    let k5 = kernel_select(&kernel_add("schema_version = 2\n\n[sandbox]\ntoolchain = \"\"\n", &s).unwrap(), "deepseek", "deepseek-v4-pro", None).unwrap();
    let t5: Table = toml::from_str(&k5).unwrap();
    assert_eq!(at(&t5, &["providers", "deepseek", "models", "deepseek-v4-pro", "prices", "standard", "input"]).as_str(), Some("1.32"),
        "no schedule: the peak price as the standard window");
}

/// The daemon paths on an installed test home: list → not ready, `keys.set` (refused for a real-looking key in
/// test mode), ready, `providers.add` + charter apply, `model.use`. The key never reaches a log or a published file.
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn requests_end_to_end_on_a_test_home() {
    let (dir, root) = installed("rq", &template());
    let p = root.p.clone();
    fs::create_dir_all(p.i("kernel")).unwrap();
    let v = list(&p).unwrap();
    assert_eq!((v["ready"].clone(), v["selected"]["provider"].clone()), (json!(false), json!("deepseek")));
    assert_eq!(v["providers"][0]["key"], "missing");
    let real = "sk-0123456789abcdefREAL";
    let mut frame = json!({"v": 1, "id": "t-1", "cmd": "keys.set", "args": {"provider": "deepseek", "secret": real}});
    let e = keys_set_req(&root, &mut frame).unwrap_err();
    assert_eq!(e.0, "keys.test_mode");
    assert_eq!(frame["args"]["secret"], Value::Null, "the secret was taken out of the request");
    let key = "test-key-uxk-0123456789";
    let mut frame = json!({"v": 1, "id": "t-2", "cmd": "keys.set", "args": {"provider": "deepseek", "secret": format!("  {key}\n")}});
    let r = keys_set_req(&root, &mut frame).unwrap();
    assert_eq!(r["stored"], true);
    let kp = p.r("keys/providers/deepseek");
    assert_eq!(fs::read(&kp).unwrap(), key.as_bytes(), "trimmed");
    assert_eq!(fs::metadata(&kp).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(list(&p).unwrap()["ready"], true);
    // add a gateway (experimental), then select it: the charter is applied, the kernel config follows
    let a = add_req(&root, &json!({"preset": "gateway", "base_url": "https://gw.example/v1", "model": "m-x", "price": "1,2"})).unwrap();
    assert_eq!((a["provider"].as_str(), a["changed"].as_bool(), a["tested"].as_str()), (Some("gateway"), Some(true), Some("experimental")));
    assert_eq!(fs::read_to_string(p.r("charter.toml")).unwrap(), fs::read_to_string(p.r("state/charter.active.toml")).unwrap(), "applied");
    let u = use_req(&root, &json!({"provider": "gateway", "model": "m-x"})).unwrap();
    assert_eq!((u["changed"].as_bool(), u["restart"].as_str()), (Some(true), Some("next_start")), "no zone runs in this test");
    let l = list(&p).unwrap();
    assert_eq!(l["selected"], json!({"provider": "gateway", "model": "m-x"}));
    assert_eq!(l["ready"], false, "the gateway has no key yet");
    assert!(use_req(&root, &json!({"provider": "nope", "model": "m-x"})).is_err());
    assert!(use_req(&root, &json!({"provider": "gateway", "model": "not-priced"})).is_err());
    // back to DeepSeek: the descriptor goes
    use_req(&root, &json!({"provider": "deepseek", "model": "deepseek-flash"})).unwrap();
    assert_eq!(list(&p).unwrap()["ready"], true);
    // the key is in exactly one file under either home
    let mut files = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let t = e.file_type().unwrap();
            if t.is_dir() { walk(&e.path(), out) } else if t.is_file() { out.push(e.path()) }
        }
    }
    walk(&dir, &mut files);
    files.retain(|f| !f.starts_with(p.r("bin"))); // the installed copy of this test binary holds the test's own literals
    let holders: Vec<_> = files.iter().filter(|f| file_holds(f, key.as_bytes()) || file_holds(f, real.as_bytes())).collect();
    assert_eq!(holders, vec![&kp], "only the 0600 key file holds the key");
    drop(root);
    let _ = fs::remove_dir_all(&dir);
}
