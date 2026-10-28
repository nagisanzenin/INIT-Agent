//! `ask_owner` (TUI-PICKER, v0.5.0): ask the owner a question with 2–4 options. The owner's
//! terminal shows it as a picker (arrow keys, Enter); the answer comes back as the owner's next
//! message (the chosen label, or their own words). The call ends the agent's turn: the loop sends
//! one extra `display` frame (`kind:"info"` with an `ask` object, [`frame`]) and then goes idle
//! without another model request (loop/mod.rs).
//!
//! The owner's side keeps every safety property (docs/tui.md §3.16): the TUI renders anything
//! that arrives with `ask` as the agent's question, caps it and strips control sequences. This
//! module only checks that the call is well formed, so the model gets a clear error instead of a
//! picker the TUI would cut down.

use serde_json::{Map, Value, json};

use super::args::{check_fields, req, type_error};
use super::{Ran, ToolError, text_result};

/// The registered name. Its schema lives in `registry.json`; this module parses the same fields.
pub const NAME: &str = "ask_owner";

pub const QUESTION_MAX: usize = 500;
pub const HEADER_MAX: usize = 24;
pub const LABEL_MAX: usize = 80;
pub const DESC_MAX: usize = 200;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 4;

/// The tool result the model reads.
pub const RESULT_TEXT: &str = "Asked the owner; your turn ends here. Their answer arrives as their next message: the chosen option's label (for multi, the labels separated by \", \"), or their own words.";
/// The `tool_end` summary.
pub const SUMMARY: &str = "asked the owner";
/// Inside a `delegate` sub-task: a sub-agent has no owner to ask.
pub const IN_SUB: &str = "error: not_available: only the main agent can ask the owner. Return the open question in your summary instead.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    pub label: String,
    pub description: Option<String>,
    pub recommended: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub question: String,
    pub header: Option<String>,
    /// The recommended option (if any) is first.
    pub options: Vec<Opt>,
    pub multi: bool,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// A trimmed string field of at most `max` characters, single line unless `lines`.
fn text_field(v: &Value, field: &str, max: usize, lines: bool) -> Result<String, ToolError> {
    let Value::String(s) = v else {
        return Err(type_error(field, "a string", v));
    };
    let s = s.trim();
    if chars(s) > max {
        return Err(ToolError::invalid(format!(
            "{field}: {} characters, at most {max}",
            chars(s)
        )));
    }
    if s.chars().any(|c| c.is_control() && !(lines && c == '\n')) {
        return Err(ToolError::invalid(format!(
            "{field}: control characters are not allowed{}",
            if lines { " (newlines are)" } else { "; keep it on one line" }
        )));
    }
    Ok(s.to_string())
}

fn opt_text(m: &Map<String, Value>, f: &str, field: &str, max: usize) -> Result<Option<String>, ToolError> {
    match m.get(f) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => text_field(v, field, max, false).map(|s| (!s.is_empty()).then_some(s)),
    }
}

fn opt_bool(m: &Map<String, Value>, f: &str, field: &str) -> Result<bool, ToolError> {
    match m.get(f) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(v) => Err(type_error(field, "a boolean", v)),
    }
}

pub fn parse(m: &Map<String, Value>) -> Result<Args, ToolError> {
    check_fields(m, &["header", "multi", "options", "question"], "")?;
    let question = text_field(req(m, "question", "")?, "question", QUESTION_MAX, true)?;
    if question.is_empty() {
        return Err(ToolError::invalid("question: must not be empty"));
    }
    let header = opt_text(m, "header", "header", HEADER_MAX)?;
    let multi = opt_bool(m, "multi", "multi")?;
    let list = match req(m, "options", "")? {
        Value::Array(a) => a,
        v => return Err(type_error("options", "an array", v)),
    };
    if list.len() < OPTIONS_MIN || list.len() > OPTIONS_MAX {
        return Err(ToolError::invalid(format!(
            "options: {} given, ask with {OPTIONS_MIN} to {OPTIONS_MAX}",
            list.len()
        ))
        .hint("the owner can always type their own answer; do not add an \"other\" option"));
    }
    let mut options: Vec<Opt> = Vec::with_capacity(list.len());
    for (i, o) in list.iter().enumerate() {
        let p = format!("options[{i}]");
        let Value::Object(om) = o else {
            return Err(type_error(&p, "an object", o));
        };
        check_fields(om, &["description", "label", "recommended"], &p)?;
        let label = text_field(req(om, "label", &p)?, &format!("{p}.label"), LABEL_MAX, false)?;
        if label.is_empty() {
            return Err(ToolError::invalid(format!("{p}.label: must not be empty")));
        }
        if label.starts_with('/') {
            return Err(ToolError::invalid(format!("{p}.label: must not start with '/'"))
                .hint("a label must not look like a command: the owner's answer is sent as a message"));
        }
        if options.iter().any(|x| x.label.to_lowercase() == label.to_lowercase()) {
            return Err(ToolError::invalid(format!("{p}.label: {label:?} is already an option")));
        }
        let description = opt_text(om, "description", &format!("{p}.description"), DESC_MAX)?;
        let recommended = opt_bool(om, "recommended", &format!("{p}.recommended"))?;
        options.push(Opt { label, description, recommended });
    }
    let recs = options.iter().filter(|o| o.recommended).count();
    if recs > 1 {
        return Err(ToolError::invalid(format!("options: {recs} are recommended, at most 1")));
    }
    // The recommended option goes first; the others keep their order.
    if let Some(i) = options.iter().position(|o| o.recommended) {
        let r = options.remove(i);
        options.insert(0, r);
    }
    Ok(Args { question, header, options, multi })
}

/// `display{kind:"tool_start"}` text.
pub fn start_text(a: &Args) -> String {
    match &a.header {
        Some(h) => format!("ask: {h}"),
        None => {
            let q = a.question.lines().next().unwrap_or("");
            let mut s: String = q.chars().take(60).collect();
            if chars(q) > 60 || a.question.contains('\n') {
                s.push('…');
            }
            format!("ask: {s}")
        }
    }
}

/// The text a client that does not know `ask` shows (`init-root attach`, older TUIs).
pub fn fallback(a: &Args) -> String {
    let mut s = format!("The agent asks: {}\n", a.question);
    for (i, o) in a.options.iter().enumerate() {
        s.push_str(&format!("  {}. {}", i + 1, o.label));
        if let Some(d) = &o.description {
            s.push_str(&format!(" — {d}"));
        }
        s.push('\n');
    }
    s.push_str("Reply with a number, the option, or your own words.");
    s
}

/// The extra `display` frame (sorted keys by `Value`, `"v":1`): `kind:"info"`, the fallback
/// text, and the `ask` object the TUI renders as a picker.
pub fn frame(a: &Args) -> Value {
    let options: Vec<Value> = a
        .options
        .iter()
        .map(|o| {
            let mut v = json!({"label": o.label});
            if let Some(d) = &o.description {
                v["description"] = d.clone().into();
            }
            if o.recommended {
                v["recommended"] = true.into();
            }
            v
        })
        .collect();
    let mut ask = json!({"multi": a.multi, "options": options, "question": a.question});
    if let Some(h) = &a.header {
        ask["header"] = h.clone().into();
    }
    json!({"ask": ask, "kind": "info", "t": "display", "text": fallback(a), "v": 1})
}

/// Runs the call: nothing to do here (the loop sends [`frame`] and ends the turn).
pub fn run(call_id: &str) -> Ran {
    Ran {
        result: text_result(call_id, RESULT_TEXT.to_string()),
        summary: SUMMARY.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: Value) -> Result<Args, ToolError> {
        parse(v.as_object().unwrap())
    }
    fn err(v: Value) -> String {
        p(v).unwrap_err().text()
    }
    fn two() -> Value {
        json!([{"label": "Postgres"}, {"label": "SQLite"}])
    }

    #[test]
    fn parses_and_moves_the_recommended_option_first() {
        let a = p(json!({"question": " Which database? ", "header": "Database", "multi": false,
            "options": [{"label": "SQLite", "description": "one file"}, {"label": "Postgres", "recommended": true}]}))
        .unwrap();
        assert_eq!(a.question, "Which database?");
        assert_eq!(a.header.as_deref(), Some("Database"));
        assert_eq!(a.options[0], Opt { label: "Postgres".into(), description: None, recommended: true });
        assert_eq!(a.options[1].label, "SQLite");
        assert!(!a.multi);
        let m = p(json!({"question": "q", "options": two(), "multi": true})).unwrap();
        assert!(m.multi && m.header.is_none());
    }

    #[test]
    fn rejects_bad_calls() {
        assert!(err(json!({"question": "q", "options": [{"label": "a"}]})).contains("1 given, ask with 2 to 4"));
        let five: Vec<Value> = (0..5).map(|i| json!({"label": format!("o{i}")})).collect();
        assert!(err(json!({"question": "q", "options": five})).contains("5 given"));
        assert!(err(json!({"question": "  ", "options": two()})).contains("question: must not be empty"));
        assert!(err(json!({"question": "q".repeat(501), "options": two()})).contains("at most 500"));
        assert!(err(json!({"question": "q", "header": "h".repeat(25), "options": two()})).contains("header: 25 characters, at most 24"));
        assert!(err(json!({"question": "q", "options": [{"label": "x".repeat(81)}, {"label": "b"}]})).contains("options[0].label: 81 characters"));
        assert!(err(json!({"question": "q", "options": [{"label": "a", "description": "d".repeat(201)}, {"label": "b"}]})).contains("at most 200"));
        let e = err(json!({"question": "q", "options": [{"label": "/full on confirm"}, {"label": "b"}]}));
        assert!(e.contains("must not start with '/'") && e.contains("hint: a label must not look like a command"), "{e}");
        assert!(err(json!({"question": "q", "options": [{"label": "Yes"}, {"label": "yes"}]})).contains("already an option"));
        assert!(err(json!({"question": "q", "options": [{"label": "a\nb"}, {"label": "c"}]})).contains("control characters"));
        assert!(err(json!({"question": "q", "options": [{"label": "a\u{1b}[2J"}, {"label": "c"}]})).contains("control characters"));
        assert!(err(json!({"question": "q", "options": two(), "choices": []})).contains("unknown field \"choices\""));
        assert!(err(json!({"question": "q", "options": [{"label": "a", "value": 1}, {"label": "b"}]})).contains("options[0]: unknown field \"value\""));
        assert!(err(json!({"question": "q", "options": [{"label": "a", "recommended": true}, {"label": "b", "recommended": true}]})).contains("2 are recommended"));
        assert!(err(json!({"question": "q", "options": "a,b"})).contains("options: expected an array"));
        assert!(err(json!({"question": "q", "options": two(), "multi": "yes"})).contains("multi: expected a boolean"));
        assert!(err(json!({"options": two()})).contains("question: missing required field"));
        // a multi-line question is fine
        assert!(p(json!({"question": "line one\nline two", "options": two()})).is_ok());
    }

    #[test]
    fn frame_is_exact() {
        let a = p(json!({"question": "Which database?", "header": "Database",
            "options": [{"label": "SQLite", "description": "one file, no server"}, {"label": "Postgres", "recommended": true}]}))
        .unwrap();
        let f = frame(&a);
        let want = json!({"ask": {"header": "Database", "multi": false, "question": "Which database?",
                "options": [{"label": "Postgres", "recommended": true}, {"description": "one file, no server", "label": "SQLite"}]},
            "kind": "info", "t": "display", "v": 1,
            "text": "The agent asks: Which database?\n  1. Postgres\n  2. SQLite — one file, no server\nReply with a number, the option, or your own words."});
        assert_eq!(f, want);
        // ICJ: sorted keys, as the kernel reads it
        assert_eq!(
            crate::log::icj::to_string(&f).unwrap(),
            r#"{"ask":{"header":"Database","multi":false,"options":[{"label":"Postgres","recommended":true},{"description":"one file, no server","label":"SQLite"}],"question":"Which database?"},"kind":"info","t":"display","text":"The agent asks: Which database?\n  1. Postgres\n  2. SQLite — one file, no server\nReply with a number, the option, or your own words.","v":1}"#
        );
        // no header, multi
        let m = p(json!({"question": "Which checks?", "multi": true, "options": two()})).unwrap();
        let f = frame(&m);
        assert_eq!(f["ask"], json!({"multi": true, "options": [{"label": "Postgres"}, {"label": "SQLite"}], "question": "Which checks?"}));
        assert_eq!(start_text(&m), "ask: Which checks?");
        assert_eq!(start_text(&a), "ask: Database");
        let long = p(json!({"question": "x".repeat(70), "options": two()})).unwrap();
        assert_eq!(start_text(&long), format!("ask: {}…", "x".repeat(60)));
    }

    #[test]
    fn run_ends_with_the_result_text() {
        let r = run("c1");
        assert!(!r.result.is_error);
        assert_eq!(r.summary, "asked the owner");
        assert_eq!(r.result.call_id, "c1");
    }
}
