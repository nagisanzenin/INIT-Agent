//! Frame envelopes shared by the initctl socket (kernel/initctl.md §3.1–§3.2) and the Root's
//! control link (root/root.md §4.1.1).
//!
//! - request: `{"args","cmd","id","v":1}`
//! - final response: `{"id","ok":true,"result","v":1}` or `{"error":{…},"id","ok":false,"v":1}`
//! - progress (initctl only, never an `ok` key): `{"id","progress":{…},"v":1}`
//! - event (control link only, no id, no response): `{"data","ev","v":1}`

use crate::errors::{self, ErrorCode};
use crate::icj::{self, IcjError, V1};
use crate::initctl::Progress;
use crate::validate;
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

/// A request line is at most 64 KiB (excluding the newline). Longer → `E_PROTO`, then close.
pub const MAX_REQUEST_LINE: usize = 64 * 1024;
/// A response, progress or control-link line is at most 1 MiB.
pub const MAX_RESPONSE_LINE: usize = 1024 * 1024;
/// `error.message` is one line of at most 500 bytes.
pub const MAX_MESSAGE_BYTES: usize = 500;
/// At most 8 concurrent connections per lane; the 9th gets `E_RATE` and is closed.
pub const MAX_CONNECTIONS_PER_LANE: usize = 8;
/// Token bucket per lane: 10 requests per second, burst 20.
pub const RATE_PER_SEC: u32 = 10;
/// Token-bucket burst.
pub const RATE_BURST: u32 = 20;
/// Idle connections are closed after 30 s without a request.
pub const IDLE_TIMEOUT_S: u64 = 30;
/// Progress frames: at most one per second while nothing changes.
pub const PROGRESS_MIN_INTERVAL_MS: u64 = 1000;

/// One entry of `error.details` (multi-problem errors, e.g. the evolution validator).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub detail: String,
}

/// The `error` object of a failed final response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    /// Always written (possibly empty), as CONVENTIONS §4 shows `details[]`.
    #[serde(default)]
    pub details: Vec<ErrorDetail>,
    /// Machine-readable context; omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorBody {
    /// A new error; the message is flattened to one line and capped at 500 bytes.
    pub fn new(code: ErrorCode, message: impl AsRef<str>) -> ErrorBody {
        ErrorBody {
            code,
            message: validate::one_line(message.as_ref(), MAX_MESSAGE_BYTES),
            details: Vec::new(),
            data: None,
        }
    }

    /// Attach `data` (any serializable object).
    pub fn with_data<T: Serialize>(mut self, data: &T) -> ErrorBody {
        self.data = Some(serde_json::to_value(data).unwrap_or(Value::Null));
        self
    }

    /// Attach `details`.
    pub fn with_details(mut self, details: Vec<ErrorDetail>) -> ErrorBody {
        self.details = details;
        self
    }

    /// `E_LEVEL` with `data: {need, have}`.
    pub fn level(need: u8, have: u8) -> ErrorBody {
        ErrorBody::new(ErrorCode::Level, format!("needs autonomy level L{need}; the level is L{have}"))
            .with_data(&errors::LevelData { need, have })
    }

    /// `E_CEILING` with `data: {ceiling, limit, used}`.
    pub fn ceiling(ceiling: &str, limit: u64, used: u64) -> ErrorBody {
        ErrorBody::new(ErrorCode::Ceiling, format!("Root ceiling {ceiling} would be exceeded ({used} of {limit} used)"))
            .with_data(&errors::CeilingData { ceiling: ceiling.to_string(), limit, used })
    }

    /// `E_ROOT` with `data.reason`.
    pub fn root(reason: errors::RootReason, message: impl AsRef<str>) -> ErrorBody {
        ErrorBody::new(ErrorCode::Root, message).with_data(&errors::RootData { reason: Some(reason), root_code: None })
    }

    /// `E_BUSY` with `data: {job?, reason?}`.
    pub fn busy(job: Option<&str>, reason: Option<errors::BusyReason>, message: impl AsRef<str>) -> ErrorBody {
        ErrorBody::new(ErrorCode::Busy, message)
            .with_data(&errors::BusyData { job: job.map(str::to_string), reason })
    }

    /// The CLI exit code for this error.
    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }

    /// Decode `data` into a typed struct.
    pub fn data_as<T: DeserializeOwned>(&self) -> Option<T> {
        self.data.clone().and_then(|d| serde_json::from_value(d).ok())
    }
}

impl fmt::Display for ErrorBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// A request frame: `{"args","cmd","id","v":1}`. `args` defaults to `{}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub v: V1,
    pub id: String,
    pub cmd: String,
    #[serde(default = "empty_object")]
    pub args: Value,
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// A request the server must refuse before dispatch, with the id to echo (null when the id
/// itself was unreadable).
#[derive(Clone, Debug, PartialEq)]
pub struct RequestReject {
    pub id: Option<String>,
    pub error: ErrorBody,
}

impl RequestReject {
    fn new(id: Option<String>, code: ErrorCode, msg: impl AsRef<str>) -> RequestReject {
        RequestReject { id, error: ErrorBody::new(code, msg) }
    }

    /// The final response to send.
    pub fn response(&self) -> Response {
        Response::err(self.id.clone(), self.error.clone())
    }

    /// `E_PROTO` closes the connection after the response (§3.1).
    pub fn closes_connection(&self) -> bool {
        self.error.code == ErrorCode::Proto
    }
}

impl Request {
    /// A request with `args` built from any serializable object.
    pub fn new<A: Serialize>(id: impl Into<String>, cmd: impl Into<String>, args: &A) -> Result<Request, IcjError> {
        Ok(Request { v: V1, id: id.into(), cmd: cmd.into(), args: icj::to_value(args)? })
    }

    /// The ICJ line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        icj::to_line(self).expect("a request is always ICJ")
    }

    /// Parse one request line (without the newline), in the server's check order:
    /// size and JSON (`E_PROTO`), `v` (`E_VERSION`), `id`, `cmd` (`E_PROTO`), `args` (`E_ARGS`).
    /// The command name itself is checked by [`crate::initctl::Command::parse`] (`E_UNKNOWN_CMD`).
    pub fn parse_line(line: &[u8]) -> Result<Request, RequestReject> {
        if line.len() > MAX_REQUEST_LINE {
            return Err(RequestReject::new(None, ErrorCode::Proto, "request line longer than 64 KiB"));
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let v: Value = serde_json::from_slice(line)
            .map_err(|e| RequestReject::new(None, ErrorCode::Proto, format!("not JSON: {e}")))?;
        let Value::Object(mut m) = v else {
            return Err(RequestReject::new(None, ErrorCode::Proto, "a request must be a JSON object"));
        };
        let id = match m.get("id") {
            Some(Value::String(s)) if validate::is_request_id(s) => Some(s.clone()),
            _ => None,
        };
        match m.get("v") {
            Some(x) if x.as_u64() == Some(1) => {}
            Some(x) => {
                return Err(RequestReject::new(id, ErrorCode::Version, format!("unsupported protocol version v={x}; this kernel speaks v=1")));
            }
            None => return Err(RequestReject::new(id, ErrorCode::Proto, "missing field `v`")),
        }
        let Some(id) = id else {
            return Err(RequestReject::new(None, ErrorCode::Proto, "`id` must be 1-64 bytes of [A-Za-z0-9._:-]"));
        };
        let cmd = match m.remove("cmd") {
            Some(Value::String(s)) => s,
            _ => return Err(RequestReject::new(Some(id), ErrorCode::Proto, "`cmd` must be a string")),
        };
        let args = match m.remove("args") {
            None | Some(Value::Null) => empty_object(),
            Some(a @ Value::Object(_)) => a,
            Some(_) => return Err(RequestReject::new(Some(id), ErrorCode::Args, "`args` must be an object")),
        };
        let d = icj::depth(&args) + 1;
        if d > icj::MAX_DEPTH {
            return Err(RequestReject::new(Some(id), ErrorCode::Args, format!("args nested {d} deep; the cap is {}", icj::MAX_DEPTH)));
        }
        Ok(Request { v: V1, id, cmd, args })
    }
}

/// A final response. `id` is null only when the request's id could not be read (e.g. `E_PEER`
/// before any request, or `E_PROTO` on garbage).
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: Option<String>,
    pub result: Result<Value, ErrorBody>,
}

impl Response {
    /// Success with a typed result.
    pub fn ok<R: Serialize>(id: impl Into<String>, result: &R) -> Result<Response, IcjError> {
        Ok(Response { id: Some(id.into()), result: Ok(icj::to_value(result)?) })
    }

    /// Success with a `Value` result.
    pub fn ok_value(id: impl Into<String>, result: Value) -> Response {
        Response { id: Some(id.into()), result: Ok(result) }
    }

    /// Failure.
    pub fn err(id: Option<String>, error: ErrorBody) -> Response {
        Response { id, result: Err(error) }
    }

    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// Decode the result into a typed struct.
    pub fn result_as<T: DeserializeOwned>(&self) -> Result<T, FrameError> {
        match &self.result {
            Ok(v) => serde_json::from_value(v.clone()).map_err(|e| FrameError::Malformed(e.to_string())),
            Err(e) => Err(FrameError::Malformed(format!("response is an error: {e}"))),
        }
    }

    /// The frame as a `Value` (keys sorted on output).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("v".into(), Value::from(1u32));
        m.insert("id".into(), self.id.clone().map_or(Value::Null, Value::String));
        match &self.result {
            Ok(r) => {
                m.insert("ok".into(), Value::Bool(true));
                m.insert("result".into(), r.clone());
            }
            Err(e) => {
                m.insert("ok".into(), Value::Bool(false));
                m.insert("error".into(), serde_json::to_value(e).expect("error body serializes"));
            }
        }
        Value::Object(m)
    }

    /// The ICJ line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        icj::to_line(&self.to_value()).expect("a response is always ICJ")
    }

    /// Parse a response object (unknown fields ignored).
    pub fn from_value(v: Value) -> Result<Response, FrameError> {
        let Value::Object(mut m) = v else {
            return Err(FrameError::Malformed("a frame must be a JSON object".into()));
        };
        check_v(&m)?;
        let id = match m.remove("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s),
            Some(_) => return Err(FrameError::Malformed("`id` must be a string or null".into())),
        };
        match m.remove("ok") {
            Some(Value::Bool(true)) => Ok(Response { id, result: Ok(m.remove("result").unwrap_or(Value::Null)) }),
            Some(Value::Bool(false)) => {
                let e = m.remove("error").ok_or_else(|| FrameError::Malformed("`ok:false` without `error`".into()))?;
                let e: ErrorBody = serde_json::from_value(e).map_err(|e| FrameError::Malformed(format!("error object: {e}")))?;
                Ok(Response { id, result: Err(e) })
            }
            _ => Err(FrameError::Malformed("`ok` must be a boolean".into())),
        }
    }
}

impl Serialize for Response {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Response::from_value(Value::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// A progress frame: `{"id","progress":{…},"v":1}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressFrame {
    pub v: V1,
    pub id: String,
    pub progress: Progress,
}

impl ProgressFrame {
    pub fn new(id: impl Into<String>, progress: Progress) -> ProgressFrame {
        ProgressFrame { v: V1, id: id.into(), progress }
    }

    /// The ICJ line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        icj::to_line(self).expect("a progress frame is always ICJ")
    }
}

/// What a client reads on an initctl connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Progress(ProgressFrame),
    Final(Response),
}

/// Why a frame could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Longer than the line limit.
    TooLong,
    /// Not JSON.
    NotJson(String),
    /// `v` other than 1.
    Version(String),
    /// JSON, but not a frame of this protocol.
    Malformed(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooLong => write!(f, "frame longer than the line limit"),
            FrameError::NotJson(e) => write!(f, "not JSON: {e}"),
            FrameError::Version(e) => write!(f, "{e}"),
            FrameError::Malformed(e) => write!(f, "malformed frame: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

fn check_v(m: &Map<String, Value>) -> Result<(), FrameError> {
    match m.get("v") {
        Some(x) if x.as_u64() == Some(1) => Ok(()),
        Some(x) => Err(FrameError::Version(format!("{}: v={x}, expected 1", icj::UNSUPPORTED_VERSION))),
        None => Err(FrameError::Malformed("missing field `v`".into())),
    }
}

fn parse_object(line: &[u8], max: usize) -> Result<Map<String, Value>, FrameError> {
    if line.len() > max {
        return Err(FrameError::TooLong);
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(FrameError::Malformed("a frame must be a JSON object".into())),
        Err(e) => Err(FrameError::NotJson(e.to_string())),
    }
}

impl Frame {
    /// Parse one line read from an initctl socket (client side).
    pub fn parse_line(line: &[u8]) -> Result<Frame, FrameError> {
        let m = parse_object(line, MAX_RESPONSE_LINE)?;
        check_v(&m)?;
        if m.contains_key("ok") {
            Response::from_value(Value::Object(m)).map(Frame::Final)
        } else if m.contains_key("progress") {
            serde_json::from_value(Value::Object(m))
                .map(Frame::Progress)
                .map_err(|e| FrameError::Malformed(format!("progress frame: {e}")))
        } else {
            Err(FrameError::Malformed("neither `ok` nor `progress`".into()))
        }
    }

    /// The ICJ line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        match self {
            Frame::Progress(p) => p.to_line(),
            Frame::Final(r) => r.to_line(),
        }
    }
}

/// A control-link event: `{"data","ev","v":1}` (root/root.md §4.1.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub v: V1,
    pub ev: String,
    #[serde(default)]
    pub data: Value,
}

/// What either side reads on the control link: requests, responses and events.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkFrame {
    Request(Request),
    Response(Response),
    Event(Event),
}

impl LinkFrame {
    /// Parse one control-link line (≤ 1 MiB).
    pub fn parse_line(line: &[u8]) -> Result<LinkFrame, FrameError> {
        let m = parse_object(line, MAX_RESPONSE_LINE)?;
        check_v(&m)?;
        let obj = Value::Object(m);
        let has = |k: &str| obj.get(k).is_some();
        if has("ev") {
            serde_json::from_value(obj).map(LinkFrame::Event).map_err(|e| FrameError::Malformed(format!("event: {e}")))
        } else if has("ok") {
            Response::from_value(obj).map(LinkFrame::Response)
        } else if has("cmd") {
            serde_json::from_value(obj).map(LinkFrame::Request).map_err(|e| FrameError::Malformed(format!("request: {e}")))
        } else {
            Err(FrameError::Malformed("neither `ev`, `ok` nor `cmd`".into()))
        }
    }

    /// The ICJ line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        match self {
            LinkFrame::Request(r) => r.to_line(),
            LinkFrame::Response(r) => r.to_line(),
            LinkFrame::Event(e) => icj::to_line(e).expect("an event is always ICJ"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_checks_in_order() {
        let ok = Request::parse_line(br#"{"args":{"ref":"evo/pty-tool"},"cmd":"promote","id":"48121-1","v":1}"#).unwrap();
        assert_eq!(ok.cmd, "promote");
        assert_eq!(ok.to_line(), b"{\"args\":{\"ref\":\"evo/pty-tool\"},\"cmd\":\"promote\",\"id\":\"48121-1\",\"v\":1}\n".to_vec());

        let e = Request::parse_line(b"garbage").unwrap_err();
        assert!(e.closes_connection());
        assert_eq!((e.id, e.error.code), (None, ErrorCode::Proto));

        // IC-3: v:2 → E_VERSION, id still echoed; the response itself is v:1.
        let e = Request::parse_line(br#"{"args":{},"cmd":"ping","id":"7","v":2}"#).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Version);
        assert_eq!(e.id.as_deref(), Some("7"));
        assert_eq!(
            String::from_utf8(e.response().to_line()).unwrap(),
            format!("{{\"error\":{{\"code\":\"E_VERSION\",\"details\":[],\"message\":\"{}\"}},\"id\":\"7\",\"ok\":false,\"v\":1}}\n", e.error.message)
        );

        let e = Request::parse_line(br#"{"cmd":"ping","id":"bad id","v":1}"#).unwrap_err();
        assert_eq!((e.id, e.error.code), (None, ErrorCode::Proto));
        let e = Request::parse_line(br#"{"args":[],"cmd":"ping","id":"1","v":1}"#).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Args);
        // Missing args is `{}`.
        assert_eq!(Request::parse_line(br#"{"cmd":"ping","id":"1","v":1}"#).unwrap().args, json!({}));
    }

    #[test]
    fn request_line_limit() {
        // IC-2: 64 KiB + 1 → E_PROTO.
        let pad = "x".repeat(MAX_REQUEST_LINE);
        let line = format!(r#"{{"args":{{"p":"{pad}"}},"cmd":"ping","id":"1","v":1}}"#);
        assert!(line.len() > MAX_REQUEST_LINE);
        let e = Request::parse_line(line.as_bytes()).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::Proto);
    }

    #[test]
    fn request_depth_cap() {
        let mut a = json!(1);
        for _ in 0..32 {
            a = json!([a]);
        }
        let line = serde_json::to_vec(&json!({"args": {"x": a}, "cmd": "ping", "id": "1", "v": 1})).unwrap();
        assert_eq!(Request::parse_line(&line).unwrap_err().error.code, ErrorCode::Args);
    }

    #[test]
    fn peer_error_has_null_id() {
        let r = Response::err(None, ErrorBody::new(ErrorCode::Peer, "peer not attributable to this lane"));
        let s = String::from_utf8(r.to_line()).unwrap();
        assert_eq!(s, "{\"error\":{\"code\":\"E_PEER\",\"details\":[],\"message\":\"peer not attributable to this lane\"},\"id\":null,\"ok\":false,\"v\":1}\n");
        let back = Frame::parse_line(s.as_bytes()).unwrap();
        assert_eq!(back, Frame::Final(r));
    }

    #[test]
    fn frame_kinds() {
        assert!(matches!(Frame::parse_line(br#"{"id":"1","ok":true,"result":{},"v":1}"#), Ok(Frame::Final(_))));
        assert!(matches!(Frame::parse_line(br#"{"id":"1","ok":true,"result":{},"v":2}"#), Err(FrameError::Version(_))));
        assert!(matches!(Frame::parse_line(br#"{"id":"1","v":1}"#), Err(FrameError::Malformed(_))));
        assert!(matches!(Frame::parse_line(b"[1]"), Err(FrameError::Malformed(_))));
        assert!(matches!(Frame::parse_line(b"{"), Err(FrameError::NotJson(_))));
        // Unknown response fields are ignored by clients.
        assert!(matches!(Frame::parse_line(br#"{"extra":1,"id":"1","ok":true,"result":{},"v":1}"#), Ok(Frame::Final(_))));
    }

    #[test]
    fn message_capped_one_line() {
        let e = ErrorBody::new(ErrorCode::Internal, format!("a\nb{}", "é".repeat(400)));
        assert!(e.message.len() <= MAX_MESSAGE_BYTES);
        assert!(!e.message.contains('\n'));
    }

    #[test]
    fn typed_error_data() {
        let e = ErrorBody::level(1, 0);
        assert_eq!(serde_json::to_value(&e).unwrap()["data"], json!({"have": 0, "need": 1}));
        assert_eq!(e.data_as::<errors::LevelData>(), Some(errors::LevelData { need: 1, have: 0 }));
        let c = ErrorBody::ceiling("parallel_bodies", 1, 1);
        assert_eq!(c.exit_code(), 77);
        assert_eq!(serde_json::to_value(&c).unwrap()["data"], json!({"ceiling": "parallel_bodies", "limit": 1, "used": 1}));
        let r = ErrorBody::root(errors::RootReason::OutboxBacklog, "outbox backed up");
        assert_eq!(r.data, Some(json!({"reason": "outbox_backlog"})));
    }

    #[test]
    fn link_frames() {
        let l = LinkFrame::parse_line(br#"{"data":{"mode":"safe"},"ev":"mode","v":1}"#).unwrap();
        assert!(matches!(l, LinkFrame::Event(ref e) if e.ev == "mode"));
        let l = LinkFrame::parse_line(br#"{"args":{},"cmd":"ready","id":"k-2","v":1}"#).unwrap();
        assert!(matches!(l, LinkFrame::Request(ref r) if r.id == "k-2"));
        let l = LinkFrame::parse_line(br#"{"id":"k-2","ok":true,"result":{},"v":1}"#).unwrap();
        assert!(matches!(l, LinkFrame::Response(_)));
    }
}
