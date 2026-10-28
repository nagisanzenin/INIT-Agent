//! UX-KEYMODEL (v0.2.0-preview): the first-run setup, `/key`, `/model` and `/setup`, from the owner's side. The
//! Root owns keys and the charter (root `providers.rs`): this flow only asks and sends `providers.list`,
//! `providers.add`, `keys.set` and `model.use` on its own request connections, never on the attach connection,
//! never to the kernel or the agent.
//!
//! First run: when `providers.list` says the selected model has no usable key, the owner picks a provider, its URL
//! when it needs one, the key (hidden input) and a model; then the Root applies it and restarts the agent. A Root
//! that does not know `providers.list` (older) answers `root.unknown_cmd`, and nothing changes.
//!
//! A pure state machine like `access.rs`: lines, secrets and answers in, [`Out`]s for the front end. The secret is
//! handed back in [`Out::SendKey`] for the front end to frame, send and zero; it is never in an `Info` text.
//!
//! TUI-PICKER (docs/tui.md §3.7.2): in TTY mode ([`Flow::with_pickers`]) every list choice (the provider, the
//! model, `/model`, `/key`'s provider) is an [`Out::Pick`]: the owner moves with the arrow keys and presses Enter,
//! and the front end hands the choice back with [`Flow::on_pick`]. Line mode keeps the numbered prompts, so
//! scripts and pipes work as before. Free-text fields (the URL, the model id, the price, the hidden key) stay
//! text inputs.

use crate::conn::ReqErr;
use crate::render::picker::{Opt, Picker, Who};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    List,
    Add,
    Use,
    Key,
}

impl Op {
    pub fn cmd(&self) -> &'static str {
        match self {
            Op::List => "providers.list",
            Op::Add => "providers.add",
            Op::Use => "model.use",
            Op::Key => "keys.set",
        }
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(if *self == Op::List { 10 } else { 60 })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub op: Op,
    pub args: Value,
}

#[derive(Debug, PartialEq)]
pub enum Out {
    Send(Req),
    /// `keys.set{provider, secret}`: the front end builds the frame, sends it, and zeroes every copy.
    SendKey { provider: String, secret: Vec<u8> },
    Info(String),
    Warn(String),
    Error(String),
    /// Secret input on (the box shows only a mask) or off.
    Secret(bool),
    /// TUI-PICKER: open this picker; its choice comes back through [`Flow::on_pick`], Esc through [`Flow::cancel`].
    Pick(Picker),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    FirstRun,
    Setup,
    Model,
    Key,
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Idle,
    Provider,
    Url,
    Key,
    Model,
    ModelId,
    Price,
    /// `/model` showed its numbered list; the next line picks.
    Pick(Vec<(String, String)>),
    /// TUI-PICKER: `/key` with no provider and several configured: the picker chooses whose key.
    KeyWho(Vec<String>),
}

#[derive(Clone, Debug, Default)]
struct Choice {
    preset: String,
    display: String,
    alias: String,
    tested: bool,
    url: Option<String>,
    default_url: Option<String>,
    needs_key: bool,
    models: Vec<String>,
    /// The catalog's prices of `models`, for the model picker's descriptions.
    model_desc: Vec<String>,
    model: Option<String>,
    price: Option<String>,
}

#[derive(Debug)]
pub struct Flow {
    /// The key field can be hidden: TTY mode, or line mode reading a pipe (nothing echoes it). Line mode on a
    /// terminal cannot hide it, so the owner stores the key with `init-root keys set` instead.
    can_hide: bool,
    checked: bool,
    step: Step,
    inflight: Option<(Op, Why)>,
    why: Why,
    arg: Option<String>,
    ch: Choice,
    /// `/key`'s provider (standalone, not in the first-run flow).
    key_only: Option<String>,
    catalog: Vec<Value>,
    /// TUI-PICKER: list choices are pickers (TTY mode), not numbered prompts (line mode).
    pickers: bool,
}

fn err_text(e: &ReqErr) -> String {
    match e {
        ReqErr::Unreachable(e) => format!("the Root is unreachable ({e})"),
        ReqErr::Proto(m) => m.clone(),
        ReqErr::Refused(c, m, _) => format!("{c}: {m}"),
    }
}

fn exp(tested: &str) -> &'static str {
    if tested == "live" { "live-tested" } else { "experimental (not live-tested)" }
}

impl Flow {
    pub fn new(can_hide: bool) -> Flow {
        Flow { can_hide, checked: false, step: Step::Idle, inflight: None, why: Why::FirstRun, arg: None, ch: Choice::default(), key_only: None, catalog: vec![], pickers: false }
    }

    /// TTY mode: list choices are pickers ([`Out::Pick`]).
    pub fn with_pickers(mut self) -> Flow {
        self.pickers = true;
        self
    }

    /// TUI-PICKER: the owner chose the option `id` in the picker this flow opened (the same as typing its
    /// number in line mode).
    pub fn on_pick(&mut self, id: usize) -> Vec<Out> {
        if self.inflight.is_some() {
            return vec![];
        }
        match self.step.clone() {
            Step::KeyWho(list) => {
                let Some(p) = list.get(id).cloned() else { return vec![] };
                self.step = Step::Idle;
                self.key_only = Some(p);
                self.ask_key()
            }
            Step::Provider | Step::Model | Step::Pick(_) => self.answer(&(id + 1).to_string()),
            _ => vec![],
        }
    }

    /// The next owner line belongs to this flow (or waits for its answer).
    #[cfg(test)]
    pub fn holds_lines(&self) -> bool {
        self.step != Step::Idle || self.inflight.is_some()
    }
    pub fn secret_mode(&self) -> bool {
        self.step == Step::Key
    }
    pub fn busy(&self) -> bool {
        self.inflight.is_some()
    }

    fn send(&mut self, op: Op, why: Why, args: Value) -> Out {
        self.inflight = Some((op, why));
        Out::Send(Req { op, args })
    }

    /// After the first attach: one `providers.list` to decide on the first run.
    pub fn on_attached(&mut self) -> Vec<Out> {
        if self.checked {
            return vec![];
        }
        self.checked = true;
        vec![self.send(Op::List, Why::FirstRun, json!({}))]
    }

    /// `/key [provider]`, `/model [n|provider/model]`, `/setup`, and every line while the flow asks. None: not ours.
    pub fn on_line(&mut self, text: &str) -> Option<Vec<Out>> {
        let t = text.trim();
        if self.inflight.is_some() {
            return Some(vec![Out::Warn("waiting for the Root; your line was not sent".into())]);
        }
        if self.step != Step::Idle {
            if matches!(t, "/skip" | "/cancel") {
                return Some(self.cancel());
            }
            return Some(self.answer(t));
        }
        let (word, rest) = t.split_once(char::is_whitespace).map_or((t, ""), |(w, r)| (w, r.trim()));
        let arg = (!rest.is_empty()).then(|| rest.to_string());
        let why = match word {
            "/key" => Why::Key,
            "/model" => Why::Model,
            "/setup" if arg.is_none() => Why::Setup,
            _ => return None,
        };
        self.arg = arg;
        Some(vec![self.send(Op::List, why, json!({}))])
    }

    /// Esc or `/cancel`: leave the flow; nothing more is sent.
    pub fn cancel(&mut self) -> Vec<Out> {
        let was_key = self.step == Step::Key;
        let was = std::mem::replace(&mut self.step, Step::Idle);
        self.key_only = None;
        let mut o = vec![];
        if was_key {
            o.push(Out::Secret(false));
        }
        o.push(Out::Info(match was {
            Step::Pick(_) => "model unchanged".into(),
            Step::KeyWho(_) => "no key changed".into(),
            _ => "setup left; /setup starts it again, /key sets a key, /model picks a model".into(),
        }));
        o
    }

    /// The owner pressed Enter in the hidden field. The bytes are moved into the request, never kept here.
    pub fn on_secret(&mut self, secret: Vec<u8>) -> Vec<Out> {
        if self.step != Step::Key {
            return vec![];
        }
        if secret.trim_ascii() == b"/cancel" || secret.trim_ascii() == b"/skip" {
            return self.cancel();
        }
        if secret.trim_ascii().is_empty() {
            return vec![Out::Warn("no key typed; paste it and press Enter, or Esc to leave".into())];
        }
        let provider = self.key_only.clone().unwrap_or_else(|| self.ch.alias.clone());
        self.step = Step::Idle;
        self.inflight = Some((Op::Key, if self.key_only.is_some() { Why::Key } else { self.why }));
        vec![Out::Secret(false), Out::SendKey { provider, secret }]
    }

    fn menu(&mut self, list: &Value) -> Vec<Out> {
        self.catalog = list["catalog"].as_array().cloned().unwrap_or_default();
        self.step = Step::Provider;
        if self.pickers {
            // the first live-tested provider is the recommended one, listed first
            let rec = self.catalog.iter().position(|c| c["tested"] == "live");
            let mut opts: Vec<Opt> = self.catalog.iter().enumerate().map(|(i, c)| {
                let ms: Vec<&str> = c["models"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).collect();
                let ms = if ms.is_empty() { String::new() } else { format!("{} · ", ms.join(", ")) };
                let o = Opt::new(i, c["display"].as_str().unwrap_or("")).desc(format!("{ms}{}", exp(c["tested"].as_str().unwrap_or(""))));
                if Some(i) == rec { o.tag("recommended") } else { o }
            }).collect();
            if let Some(r) = rec {
                let o = opts.remove(r);
                opts.insert(0, o);
            }
            return vec![Out::Pick(Picker::new(Who::Tui, "provider", "Choose a model provider", opts).esc("leave setup"))];
        }
        let mut o = vec![Out::Info("Choose a model provider (type a number and press Enter; Enter = 1):".into())];
        for (i, c) in self.catalog.iter().enumerate() {
            let ms: Vec<&str> = c["models"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).collect();
            let ms = if ms.is_empty() { String::new() } else { format!(" · {}", ms.join(", ")) };
            o.push(Out::Info(format!("  {}  {}{ms} · {}", i + 1, c["display"].as_str().unwrap_or(""), exp(c["tested"].as_str().unwrap_or("")))));
        }
        o.push(Out::Info("  /skip leaves setup (/setup starts it again)".into()));
        o
    }

    fn ask_key(&mut self) -> Vec<Out> {
        if !self.can_hide {
            self.step = Step::Idle;
            let a = self.key_only.take().unwrap_or_else(|| self.ch.alias.clone());
            let mut o = vec![Out::Warn(format!("this terminal would show the key: store it with `init-root keys set {a}` (hidden input) in a terminal, or run init-agent without --plain"))];
            if self.why != Why::Key {
                o.extend(self.ask_model());
            }
            return o;
        }
        self.step = Step::Key;
        let who = if self.key_only.is_some() { self.key_only.clone().unwrap_or_default() } else { self.ch.display.clone() };
        vec![Out::Secret(true), Out::Info(format!("Paste the {who} API key and press Enter. It is hidden, goes only to the Root (stored 0600), never to the agent. Esc leaves."))]
    }

    fn ask_model(&mut self) -> Vec<Out> {
        if self.ch.models.is_empty() {
            self.step = Step::ModelId;
            return vec![Out::Info("Model id, exactly as the server names it (for example qwen3-coder or vendor/model):".into())];
        }
        self.step = Step::Model;
        if self.pickers {
            let opts = self.ch.models.iter().enumerate().map(|(i, m)| {
                let o = Opt::new(i, m.as_str()).desc(self.ch.model_desc.get(i).cloned().unwrap_or_default());
                if i == 0 && self.ch.tested { o.tag("recommended") } else { o }
            }).collect();
            return vec![Out::Pick(Picker::new(Who::Tui, "model", format!("Choose a {} model", self.ch.display), opts).esc("leave setup"))];
        }
        let mut o = vec![Out::Info("Choose a model (Enter = 1):".into())];
        for (i, m) in self.ch.models.iter().enumerate() {
            o.push(Out::Info(format!("  {}  {m}", i + 1)));
        }
        o
    }

    fn apply(&mut self) -> Vec<Out> {
        self.step = Step::Idle;
        let args = json!({"preset": self.ch.preset, "base_url": self.ch.url, "model": self.ch.model, "price": self.ch.price});
        vec![Out::Info(format!("applying: {} · {}…", self.ch.display, self.ch.model.as_deref().unwrap_or(""))), self.send(Op::Add, self.why, args)]
    }

    fn answer(&mut self, t: &str) -> Vec<Out> {
        match self.step.clone() {
            Step::Provider => {
                let n = if t.is_empty() { 1 } else { t.parse::<usize>().unwrap_or(0) };
                let Some(c) = n.checked_sub(1).and_then(|i| self.catalog.get(i)).cloned() else {
                    return vec![Out::Warn(format!("type a number from 1 to {}, or /skip", self.catalog.len()))];
                };
                let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
                self.ch = Choice { preset: s("preset"), display: s("display"), alias: s("preset"), tested: c["tested"] == "live",
                    default_url: c["default_url"].as_str().map(String::from), needs_key: c["needs_key"] != false,
                    models: c["models"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect(),
                    model_desc: c["models"].as_array().into_iter().flatten().filter(|m| m["id"].is_string()).map(price).collect(), ..Choice::default() };
                let mut o = vec![];
                if !self.ch.tested {
                    o.push(Out::Warn(format!("{} is experimental: it is not live-tested with INIT yet", self.ch.display)));
                }
                if c["needs_url"] == true {
                    self.step = Step::Url;
                    o.push(Out::Info(match &self.ch.default_url {
                        Some(d) => format!("Server URL (Enter = {d}):"),
                        None => "Base URL, https://host/path (for example https://openrouter.ai/api/v1):".into(),
                    }));
                    return o;
                }
                o.extend(if self.ch.needs_key { self.ask_key() } else { self.ask_model() });
                o
            }
            Step::Url => {
                let url = if t.is_empty() { self.ch.default_url.clone() } else { Some(t.to_string()) };
                let Some(url) = url else { return vec![Out::Warn("type the base URL, or /skip".into())] };
                let ok = if self.ch.default_url.is_some() { url.starts_with("http://") } else { url.starts_with("https://") };
                if !ok {
                    return vec![Out::Warn(if self.ch.default_url.is_some() { "a local server URL starts with http://127.0.0.1:PORT" } else { "the URL must start with https://" }.into())];
                }
                self.ch.url = Some(url);
                if self.ch.needs_key { self.ask_key() } else { self.ask_model() }
            }
            Step::Model => {
                let n = if t.is_empty() { 1 } else { t.parse::<usize>().unwrap_or(0) };
                let Some(m) = n.checked_sub(1).and_then(|i| self.ch.models.get(i)).cloned() else {
                    return vec![Out::Warn(format!("type a number from 1 to {}", self.ch.models.len()))];
                };
                self.ch.model = Some(m);
                self.apply()
            }
            Step::ModelId => {
                if t.is_empty() || t.contains(char::is_whitespace) {
                    return vec![Out::Warn("type the model id (no spaces), or /skip".into())];
                }
                self.ch.model = Some(t.to_string());
                if self.ch.preset == "local" {
                    return self.apply();
                }
                self.step = Step::Price;
                vec![Out::Info("Its price in USD per 1M tokens as IN,CACHED,OUT (for example 0.50,0.05,1.50); Enter = unknown (spend then shows $0):".into())]
            }
            Step::Price => {
                if !t.is_empty() {
                    let ok = t.split(',').count() >= 2 && t.split(',').all(|x| !x.trim().is_empty() && x.trim().bytes().all(|c| c.is_ascii_digit() || c == b'.'));
                    if !ok {
                        return vec![Out::Warn("like 0.50,0.05,1.50 (IN,CACHED,OUT) or 0.50,1.50 (IN,OUT); Enter = unknown".into())];
                    }
                    self.ch.price = Some(t.to_string());
                }
                self.apply()
            }
            Step::Pick(choices) => {
                if t.is_empty() {
                    self.step = Step::Idle;
                    return vec![Out::Info("model unchanged".into())];
                }
                self.step = Step::Idle;
                match pick(&choices, t) {
                    Some((p, m)) => vec![self.send(Op::Use, Why::Model, json!({"provider": p, "model": m}))],
                    None => vec![Out::Warn(format!("{t:?} is not in the list; /model shows it again"))],
                }
            }
            Step::KeyWho(list) => {
                // line mode never opens this step; a typed provider id or number still works
                let p = t.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| list.get(i).cloned())
                    .or_else(|| list.iter().find(|p| p.as_str() == t).cloned());
                match p {
                    Some(p) => {
                        self.step = Step::Idle;
                        self.key_only = Some(p);
                        self.ask_key()
                    }
                    None => vec![Out::Warn(format!("choose one of: {}", list.join(", ")))],
                }
            }
            Step::Key | Step::Idle => vec![],
        }
    }

    pub fn on_answer(&mut self, op: Op, r: Result<Value, ReqErr>) -> Vec<Out> {
        let Some((_, why)) = self.inflight.take().filter(|x| x.0 == op) else { return vec![] };
        let v = match r {
            Ok(v) => v,
            Err(ReqErr::Refused(c, _, _)) if op == Op::List && c == "root.unknown_cmd" => {
                return if why == Why::FirstRun { vec![] } else { vec![Out::Warn("this Root does not support in-app provider setup; use init-root providers / keys set".into())] };
            }
            Err(e) if op == Op::Key => {
                // stay on the key: a refused key (test mode, format) can be typed again
                let mut o = vec![Out::Error(format!("key not stored: {}", err_text(&e)))];
                o.extend(self.ask_key());
                return o;
            }
            Err(e) => {
                self.step = Step::Idle;
                return if why == Why::FirstRun && op == Op::List { vec![] } else {
                    vec![Out::Error(format!("{}: {}", op.cmd(), err_text(&e))), Out::Info("/setup starts the setup again".into())]
                };
            }
        };
        match op {
            Op::List => self.on_list(why, &v),
            Op::Key if why == Why::Key => {
                vec![Out::Info(format!("✓ key stored for {} (never shown; the Root keeps it)", v["provider"].as_str().unwrap_or("")))]
            }
            Op::Key => {
                let mut o = vec![Out::Info(format!("✓ key stored for {}", v["provider"].as_str().unwrap_or("")))];
                o.extend(self.ask_model());
                o
            }
            Op::Add => {
                let alias = v["provider"].as_str().unwrap_or(&self.ch.alias).to_string();
                let model = v["model"].as_str().unwrap_or("").to_string();
                let mut o = vec![];
                if v["unpriced"] == true {
                    o.push(Out::Warn("no price given: this model's spend shows as $0.00 (unknown)".into()));
                }
                o.push(self.send(Op::Use, why, json!({"provider": alias, "model": model})));
                o
            }
            Op::Use => {
                let what = format!("{}/{}", v["provider"].as_str().unwrap_or(""), v["model"].as_str().unwrap_or(""));
                let tail = match (v["changed"] == true, v["restart"].as_str()) {
                    (false, _) => "already selected",
                    (true, Some("now")) => "selected; the agent restarts with it",
                    _ => "selected; it takes effect when the agent starts",
                };
                let mut o = vec![Out::Info(format!("✓ model {what}: {tail}"))];
                if why != Why::Model {
                    o.push(Out::Info("Setup done. Type a message to start. (/model switches models, /key replaces a key, /setup runs this again)".into()));
                }
                o
            }
        }
    }

    fn on_list(&mut self, why: Why, v: &Value) -> Vec<Out> {
        let sel = format!("{}/{}", v["selected"]["provider"].as_str().unwrap_or(""), v["selected"]["model"].as_str().unwrap_or(""));
        self.why = why;
        match why {
            Why::FirstRun if v["ready"] == true => vec![],
            Why::FirstRun | Why::Setup => {
                let mut o = vec![];
                if why == Why::FirstRun {
                    o.push(Out::Info(format!("Welcome to INIT. The selected model {sel} has no usable key yet, so let's set one up.")));
                }
                o.extend(self.menu(v));
                o
            }
            Why::Key => {
                // TUI-PICKER: no provider named and several configured that take a key: the picker asks whose
                let keyed: Vec<&Value> = v["providers"].as_array().into_iter().flatten().filter(|x| x["key"] != "not_required" && x["id"].is_string()).collect();
                if self.pickers && self.arg.is_none() && keyed.len() > 1 {
                    let sel = v["selected"]["provider"].as_str().unwrap_or("");
                    let ids: Vec<String> = keyed.iter().map(|x| x["id"].as_str().unwrap_or("").to_string()).collect();
                    let mut opts: Vec<Opt> = keyed.iter().enumerate().map(|(i, x)| {
                        let st = match x["key"].as_str() { Some("present") => "a key is stored", _ => "no key yet" };
                        let o = Opt::new(i, x["id"].as_str().unwrap_or("")).desc(st);
                        if x["id"] == sel { o.tag("current") } else { o }
                    }).collect();
                    if let Some(c) = opts.iter().position(|o| o.tag.is_some()) {
                        let o = opts.remove(c);
                        opts.insert(0, o);
                    }
                    self.step = Step::KeyWho(ids);
                    return vec![Out::Pick(Picker::new(Who::Tui, "key", "Set the API key of", opts).esc("cancel"))];
                }
                let p = self.arg.take().unwrap_or_else(|| v["selected"]["provider"].as_str().unwrap_or("").to_string());
                let known = v["providers"].as_array().into_iter().flatten().find(|x| x["id"] == p.as_str()).cloned();
                match known {
                    Some(k) if k["key"] == "not_required" => vec![Out::Info(format!("{p} is a local server: it needs no key"))],
                    _ if p.is_empty() => vec![Out::Warn("/key <provider>".into())],
                    k => {
                        let mut o = vec![];
                        if k.is_none() {
                            o.push(Out::Warn(format!("{p} is not a configured provider; the key is stored, and used once you add it (/setup)")));
                        }
                        self.key_only = Some(p);
                        o.extend(self.ask_key());
                        o
                    }
                }
            }
            Why::Model => {
                let choices: Vec<(String, String, Value)> = v["models"].as_array().into_iter().flatten()
                    .map(|m| (m["provider"].as_str().unwrap_or("").to_string(), m["model"].as_str().unwrap_or("").to_string(), m.clone())).collect();
                let pairs: Vec<(String, String)> = choices.iter().map(|c| (c.0.clone(), c.1.clone())).collect();
                if let Some(a) = self.arg.take() {
                    return match pick(&pairs, &a) {
                        Some((p, m)) => vec![self.send(Op::Use, Why::Model, json!({"provider": p, "model": m}))],
                        None => vec![Out::Warn(format!("{a:?} is not configured; /model lists the models, /setup adds a provider"))],
                    };
                }
                if self.pickers {
                    let mut opts: Vec<Opt> = choices.iter().enumerate().map(|(i, (p, m, x))| {
                        let key = match x["key"].as_str() { Some("present" | "not_required") => "", _ => " · no key (/key)" };
                        let t = if x["tested"] == "live" { "" } else { " · experimental (not live-tested)" };
                        let o = Opt::new(i, format!("{p}/{m}")).desc(format!("${} in · ${} out per 1M tokens{key}{t}",
                            x["input_usd"].as_str().unwrap_or("?"), x["output_usd"].as_str().unwrap_or("?")));
                        if x["selected"] == true { o.tag("current") } else { o }
                    }).collect();
                    if let Some(c) = opts.iter().position(|o| o.tag.is_some()) {
                        let o = opts.remove(c);
                        opts.insert(0, o);
                    }
                    self.step = Step::Pick(pairs);
                    return vec![Out::Pick(Picker::new(Who::Tui, "model", "Switch model (/setup adds a provider)", opts).esc("keep the current model"))];
                }
                let mut o = vec![Out::Info("Models (USD per 1M tokens in/out; * = selected):".into())];
                for (i, (p, m, x)) in choices.iter().enumerate() {
                    let star = if x["selected"] == true { "*" } else { " " };
                    let key = match x["key"].as_str() { Some("present" | "not_required") => "", _ => " · no key (/key)" };
                    let t = if x["tested"] == "live" { "" } else { " · experimental (not live-tested)" };
                    o.push(Out::Info(format!("{star} {}  {p}/{m}  ${}/${}{key}{t}", i + 1, x["input_usd"].as_str().unwrap_or("?"), x["output_usd"].as_str().unwrap_or("?"))));
                }
                o.push(Out::Info(format!("Type a number to switch (Enter keeps {sel}); /setup adds a provider.")));
                self.step = Step::Pick(pairs);
                o
            }
        }
    }
}

/// A catalog model's price, for the model picker: `$0.30 in · $1.20 out per 1M tokens`.
fn price(m: &Value) -> String {
    match (m["input_usd"].as_str(), m["output_usd"].as_str()) {
        (Some(i), Some(o)) => format!("${i} in · ${o} out per 1M tokens"),
        _ => String::new(),
    }
}

/// A number from the list, or `provider/model` as listed.
fn pick(choices: &[(String, String)], t: &str) -> Option<(String, String)> {
    if let Ok(n) = t.parse::<usize>() {
        return n.checked_sub(1).and_then(|i| choices.get(i)).cloned();
    }
    let (p, m) = t.split_once('/')?;
    choices.iter().find(|c| c.0 == p && c.1 == m).cloned()
}

/// `keys.set` as one request frame, built in a preallocated buffer the caller zeroes after writing it. The secret is
/// UTF-8 without control characters (checked here), JSON-escaped by hand so no intermediate `String` holds it.
pub fn key_frame(provider: &str, secret: &[u8]) -> Option<Vec<u8>> {
    let s = std::str::from_utf8(secret.trim_ascii()).ok()?;
    if s.is_empty() || s.len() > 512 || s.chars().any(char::is_control) {
        return None;
    }
    let mut f = Vec::with_capacity(2048);
    f.extend_from_slice(b"{\"args\":{\"provider\":");
    f.extend_from_slice(serde_json::to_string(provider).ok()?.as_bytes());
    f.extend_from_slice(b",\"secret\":\"");
    for b in s.bytes() {
        if b == b'"' || b == b'\\' {
            f.push(b'\\');
        }
        f.push(b);
    }
    f.extend_from_slice(b"\"},\"cmd\":\"keys.set\",\"id\":\"t-1\",\"v\":1}\n");
    Some(f)
}

/// Overwrites a buffer with zeros so the compiler keeps the stores (docs/ux-v1.md §3.4.5).
pub fn zero(b: &mut [u8]) {
    for x in b.iter_mut() {
        *x = 0;
    }
    std::hint::black_box(b);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(ready: bool) -> Value {
        json!({"ready": ready, "selected": {"provider": "deepseek", "model": "deepseek-flash"},
            "providers": [{"id": "deepseek", "key": if ready { "present" } else { "missing" }, "tested": "live"}],
            "models": [{"provider": "deepseek", "model": "deepseek-flash", "selected": true, "tested": "live", "key": "present", "input_usd": "0.30", "output_usd": "1.20"},
                       {"provider": "deepseek", "model": "deepseek-v4-pro", "selected": false, "tested": "live", "key": "present", "input_usd": "1.32", "output_usd": "3.96"}],
            "catalog": [{"preset": "deepseek", "display": "DeepSeek", "tested": "live", "needs_url": false, "needs_key": true, "default_url": null,
                         "models": [{"id": "deepseek-flash"}, {"id": "deepseek-v4-pro"}]},
                        {"preset": "gateway", "display": "OpenAI-compatible gateway", "tested": "experimental", "needs_url": true, "needs_key": true, "default_url": null, "models": []},
                        {"preset": "local", "display": "Local server", "tested": "experimental", "needs_url": true, "needs_key": false,
                         "default_url": "http://127.0.0.1:11434/v1", "models": []}]})
    }

    fn sent(o: &[Out]) -> Vec<(Op, Value)> {
        o.iter().filter_map(|x| if let Out::Send(r) = x { Some((r.op, r.args.clone())) } else { None }).collect()
    }

    fn texts(o: &[Out]) -> String {
        o.iter().filter_map(|x| match x { Out::Info(t) | Out::Warn(t) | Out::Error(t) => Some(t.as_str()), _ => None }).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn ready_owner_sees_nothing() {
        let mut f = Flow::new(true);
        assert_eq!(sent(&f.on_attached()), vec![(Op::List, json!({}))]);
        assert!(f.on_answer(Op::List, Ok(list(true))).is_empty(), "an existing DeepSeek user: no change");
        assert!(!f.holds_lines());
        assert!(f.on_line("hello").is_none());
        assert!(f.on_attached().is_empty(), "once per process");
        // an older Root: no first run, no noise
        let mut g = Flow::new(true);
        g.on_attached();
        assert!(g.on_answer(Op::List, Err(ReqErr::Refused("root.unknown_cmd".into(), "x".into(), Value::Null))).is_empty());
    }

    #[test]
    fn first_run_deepseek_provider_key_model() {
        let mut f = Flow::new(true);
        f.on_attached();
        let o = f.on_answer(Op::List, Ok(list(false)));
        assert!(texts(&o).contains("1  DeepSeek · deepseek-flash, deepseek-v4-pro · live-tested"), "{}", texts(&o));
        assert!(texts(&o).contains("2  OpenAI-compatible gateway · experimental (not live-tested)"));
        assert!(f.holds_lines());
        let o = f.on_line("").unwrap(); // Enter = DeepSeek
        assert!(o.contains(&Out::Secret(true)) && f.secret_mode());
        let o = f.on_secret(b"test-abc".to_vec());
        assert_eq!(o, vec![Out::Secret(false), Out::SendKey { provider: "deepseek".into(), secret: b"test-abc".to_vec() }]);
        assert!(f.on_line("x").unwrap().iter().any(|x| matches!(x, Out::Warn(_))), "lines wait for the answer");
        let o = f.on_answer(Op::Key, Ok(json!({"provider": "deepseek", "stored": true})));
        assert!(texts(&o).contains("1  deepseek-flash") && !texts(&o).contains("test-abc"));
        let o = f.on_line("2").unwrap();
        assert_eq!(sent(&o), vec![(Op::Add, json!({"preset": "deepseek", "base_url": null, "model": "deepseek-v4-pro", "price": null}))]);
        let o = f.on_answer(Op::Add, Ok(json!({"provider": "deepseek", "model": "deepseek-v4-pro", "changed": true})));
        assert_eq!(sent(&o), vec![(Op::Use, json!({"provider": "deepseek", "model": "deepseek-v4-pro"}))]);
        let o = f.on_answer(Op::Use, Ok(json!({"provider": "deepseek", "model": "deepseek-v4-pro", "changed": true, "restart": "now"})));
        assert!(texts(&o).contains("✓ model deepseek/deepseek-v4-pro: selected; the agent restarts with it"));
        assert!(!f.holds_lines());
    }

    #[test]
    fn gateway_and_local_ask_for_url_model_and_price() {
        let mut f = Flow::new(true);
        f.on_attached();
        f.on_answer(Op::List, Ok(list(false)));
        let o = f.on_line("2").unwrap();
        assert!(texts(&o).contains("experimental"));
        assert!(f.on_line("http://gw.example").unwrap().iter().any(|x| matches!(x, Out::Warn(_))), "https only");
        assert!(f.on_line("https://gw.example/api/v1").unwrap().contains(&Out::Secret(true)));
        f.on_secret(b"k".to_vec());
        f.on_answer(Op::Key, Ok(json!({"provider": "gateway"})));
        f.on_line("vendor/model-x").unwrap();
        let o = f.on_line("0.5,1.5").unwrap();
        assert_eq!(sent(&o)[0].1, json!({"preset": "gateway", "base_url": "https://gw.example/api/v1", "model": "vendor/model-x", "price": "0.5,1.5"}));
        // local: no key, the default URL
        let mut g = Flow::new(true);
        g.on_attached();
        g.on_answer(Op::List, Ok(list(false)));
        g.on_line("3").unwrap();
        let o = g.on_line("").unwrap();
        assert!(!o.contains(&Out::Secret(true)), "a local server needs no key");
        let o = g.on_line("qwen3").unwrap();
        assert_eq!(sent(&o)[0].1["base_url"], "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn key_cannot_be_hidden_in_line_mode_on_a_terminal() {
        let mut f = Flow::new(false);
        f.on_attached();
        f.on_answer(Op::List, Ok(list(false)));
        let o = f.on_line("1").unwrap();
        assert!(!o.contains(&Out::Secret(true)));
        assert!(texts(&o).contains("init-root keys set deepseek"));
        assert!(texts(&o).contains("Choose a model"), "the setup goes on with the model");
        let mut g = Flow::new(false);
        assert!(texts(&g.on_line("/key").map(|_| g.on_answer(Op::List, Ok(list(true)))).unwrap()).contains("init-root keys set deepseek"));
    }

    #[test]
    fn model_lists_and_picks() {
        let mut f = Flow::new(true);
        assert_eq!(sent(&f.on_line("/model").unwrap()), vec![(Op::List, json!({}))]);
        let o = f.on_answer(Op::List, Ok(list(true)));
        assert!(texts(&o).contains("* 1  deepseek/deepseek-flash  $0.30/$1.20"), "{}", texts(&o));
        assert_eq!(sent(&f.on_line("2").unwrap()), vec![(Op::Use, json!({"provider": "deepseek", "model": "deepseek-v4-pro"}))]);
        let mut g = Flow::new(true);
        g.on_line("/model deepseek/deepseek-v4-pro").unwrap();
        assert_eq!(sent(&g.on_answer(Op::List, Ok(list(true)))), vec![(Op::Use, json!({"provider": "deepseek", "model": "deepseek-v4-pro"}))]);
        let mut h = Flow::new(true);
        h.on_line("/model").unwrap();
        h.on_answer(Op::List, Ok(list(true)));
        assert!(texts(&h.on_line("").unwrap()).contains("unchanged"));
        assert!(h.on_line("/models").is_none(), "only the exact commands");
    }

    #[test]
    fn key_command_and_escape() {
        let mut f = Flow::new(true);
        f.on_line("/key").unwrap();
        let o = f.on_answer(Op::List, Ok(list(true)));
        assert!(o.contains(&Out::Secret(true)));
        assert_eq!(f.cancel(), vec![Out::Secret(false), Out::Info("setup left; /setup starts it again, /key sets a key, /model picks a model".into())]);
        f.on_line("/key deepseek").unwrap();
        f.on_answer(Op::List, Ok(list(true)));
        assert_eq!(f.on_secret(b"  ".to_vec()).len(), 1, "empty: asked again");
        let o = f.on_secret(b"test-x".to_vec());
        assert!(matches!(&o[1], Out::SendKey { provider, .. } if provider == "deepseek"));
        // a refused key: asked again, in the hidden field
        let o = f.on_answer(Op::Key, Err(ReqErr::Refused("keys.test_mode".into(), "only test-".into(), Value::Null)));
        assert!(o.contains(&Out::Secret(true)) && f.secret_mode());
    }

    #[test]
    fn key_frame_is_valid_json_and_escaped() {
        let f = key_frame("deepseek", b" sk-a\"b\\c \n").unwrap();
        let v: Value = serde_json::from_slice(&f).unwrap();
        assert_eq!(v, json!({"args": {"provider": "deepseek", "secret": "sk-a\"b\\c"}, "cmd": "keys.set", "id": "t-1", "v": 1}));
        assert!(key_frame("deepseek", b"a\x01b").is_none() && key_frame("deepseek", b"").is_none() && key_frame("x", &[b'a'; 513]).is_none());
        let mut b = f;
        zero(&mut b);
        assert!(b.iter().all(|x| *x == 0));
    }
}
