//! The body stdio protocol `INIT_UI=stdio-jsonl/1` (kernel/supervise.md §4.5).
//!
//! One JSON object per line, UTF-8, `\n`-terminated, ≤ 1 MiB, `"v":1` on every frame. Unknown
//! `t` values and unknown fields are ignored by both sides: an unknown `t` parses as `Unknown`.
//! Use [`encode_to_body`] / [`decode_from_kernel`] and friends, which add and check `v`.

use crate::files::UpdateFailedFile;
use crate::icj::{self, IcjError, Versioned};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum stdio line (1 MiB).
pub const MAX_LINE: usize = 1024 * 1024;
/// `stop.grace_ms` default.
pub const STOP_GRACE_MS: u64 = 30_000;
/// The exit code a body uses after `exit{code:75}` (handoff complete).
pub const HANDOFF_EXIT_CODE: i32 = 75;

/// `input.mode`: `steer` (default) is delivered into a running turn; `follow_up` waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    Steer,
    FollowUp,
}

impl InputMode {
    pub fn is_steer(&self) -> bool {
        *self == InputMode::Steer
    }
}

/// `pause.reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Handoff,
    Rollback,
    Swap,
}

/// `reload.kind`: `request` from `initctl reload`; `kept` after the kernel published kept
/// components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadKind {
    Request,
    Kept,
}

/// Notice kinds (`notice.kind`). Kept as strings: the set grows across docs.
pub mod notice_kind {
    pub const GATE: &str = "gate";
    pub const UPDATE_FAILED: &str = "update_failed";
    pub const THINNING_DONE: &str = "thinning_done";
    pub const NOTICE: &str = "notice";
    pub const LKG: &str = "lkg";
    pub const PROMOTED: &str = "promoted";
    pub const PROVISIONAL: &str = "provisional";
    pub const ACCEPTED: &str = "accepted";
    pub const BUDGET: &str = "budget";
    pub const SAFE: &str = "safe";
    pub const LEVEL: &str = "level";
    pub const AUDIT: &str = "audit";
    pub const FLEET: &str = "fleet";
    pub const KUPGRADE: &str = "kupgrade";
    pub const TASK_ADMITTED: &str = "task_admitted";
}

/// Kernel → body frames (stdin).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToBody {
    /// Append a user `message` with `input_id = id` (drop duplicates), then `ack{seq}`.
    Input {
        /// 16 hex.
        id: String,
        seq: u64,
        text: String,
        ts: String,
        /// Omitted on the wire when `steer` (the default).
        #[serde(default, skip_serializing_if = "InputMode::is_steer")]
        mode: InputMode,
    },
    /// Abort the current turn, keep partial output.
    Interrupt,
    /// Pause at the next step boundary, then send `paused`.
    Pause { token: String, reason: PauseReason },
    /// Re-lock, rescan, append `update_failed` if not null, continue.
    Resume { token: String, update_failed: Option<UpdateFailedFile> },
    /// Exit with `code` (75) within 10 s, writing nothing more to the log.
    Exit { token: String, code: i32 },
    /// Finish or abort the turn, fsync, exit 0.
    Stop { reason: String, grace_ms: u64 },
    /// T0/T1 reload at the next step boundary, then `reload_done`.
    Reload {
        job: String,
        kind: ReloadKind,
        /// For `kept`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        components: Option<Vec<String>>,
    },
    /// Append the matching event, then `notice_ack{id}`; drop duplicates by `id`.
    Notice { id: String, kind: String, data: Value },
    /// A `t` this build does not know (ignored).
    #[serde(other)]
    Unknown,
}

/// `display.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayKind {
    Text,
    Thinking,
    ToolStart,
    ToolEnd,
    TurnEnd,
    Info,
    Error,
}

/// The fields of a body `display` frame (also relayed in `owner.out`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub kind: DisplayKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// Body → kernel frames (stdout).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FromBody {
    /// Shown to the owner.
    Display(Display),
    /// Input durably in the log.
    Ack { seq: u64 },
    /// Notice durably in the log.
    NoticeAck { id: String },
    /// Sent on every busy↔idle change.
    State { busy: bool },
    /// Pause reached.
    Paused { token: String, log_len: u64, leaf: String },
    /// Completes the reload job.
    ReloadDone {
        job: String,
        ok: bool,
        prefix_changed: bool,
        components: Vec<String>,
        error: Option<String>,
    },
    /// A `t` this build does not know (ignored).
    #[serde(other)]
    Unknown,
}

/// Encode a kernel → body frame (ICJ line with `"v":1`).
pub fn encode_to_body(f: &ToBody) -> Result<Vec<u8>, IcjError> {
    icj::to_line(&Versioned(f))
}

/// Encode a body → kernel frame.
pub fn encode_from_body(f: &FromBody) -> Result<Vec<u8>, IcjError> {
    icj::to_line(&Versioned(f))
}

fn decode<T: serde::de::DeserializeOwned>(line: &[u8]) -> Result<T, IcjError> {
    if line.len() > MAX_LINE + 1 {
        return Err(IcjError::Json(format!("line longer than {MAX_LINE} bytes")));
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    icj::from_slice::<Versioned<T>>(line).map(|v| v.0)
}

/// Decode a line the body read on stdin.
pub fn decode_to_body(line: &[u8]) -> Result<ToBody, IcjError> {
    decode(line)
}

/// Decode a line the kernel read on the body's stdout.
pub fn decode_from_body(line: &[u8]) -> Result<FromBody, IcjError> {
    decode(line)
}

/// The fields of a body `display` line that [`Display`] does not know (not `t` or `v`), which the
/// kernel relays to the owner unchanged (supervise.md §4.6; tui.md §3.15, §9 Q2). Empty when the
/// line is not a valid display frame: the known fields keep [`Display`]'s validation.
pub fn display_extra(line: &[u8]) -> BTreeMap<String, Value> {
    #[derive(Deserialize)]
    struct Split {
        #[serde(flatten)]
        _known: Display,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    }
    let mut extra = decode::<Split>(line).map(|s| s.extra).unwrap_or_default();
    extra.remove("t");
    extra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_t_and_fields_ignored() {
        assert_eq!(decode_to_body(br#"{"t":"future_frame","x":1,"v":1}"#).unwrap(), ToBody::Unknown);
        assert_eq!(decode_from_body(br#"{"seq":3,"t":"ack","extra":true,"v":1}"#).unwrap(), FromBody::Ack { seq: 3 });
        assert!(decode_from_body(br#"{"seq":3,"t":"ack","v":2}"#).is_err());
        assert!(decode_from_body(br#"{"seq":3,"t":"ack"}"#).is_err());
    }

    #[test]
    fn input_mode_defaults_to_steer() {
        let f = decode_to_body(br#"{"id":"9c41e07a2b5d13f0","seq":18,"t":"input","text":"x","ts":"t","v":1}"#).unwrap();
        assert!(matches!(f, ToBody::Input { mode: InputMode::Steer, .. }));
    }

    #[test]
    fn display_extra_keeps_only_unknown_fields() {
        let line = br#"{"call_id":"c1","detail":{"lines":[1,2]},"kind":"tool_end","name":"bash","ok":true,"t":"display","text":"exit 0","v":1,"x_future":"y"}"#;
        let FromBody::Display(d) = decode_from_body(line).unwrap() else { panic!() };
        assert_eq!((d.kind, d.ok, d.call_id.as_deref()), (DisplayKind::ToolEnd, Some(true), Some("c1")));
        let x = display_extra(line);
        assert_eq!(serde_json::to_value(&x).unwrap(), serde_json::json!({"detail": {"lines": [1, 2]}, "x_future": "y"}));
        assert!(display_extra(br#"{"kind":"text","t":"display","text":"plain","v":1}"#).is_empty());
        // The known fields keep their validation: a bad one rejects the frame, extras and all.
        let bad = br#"{"detail":1,"kind":"text","ok":"yes","t":"display","text":"x","v":1}"#;
        assert!(decode_from_body(bad).is_err());
        assert!(display_extra(bad).is_empty());
    }

    #[test]
    fn unit_frames() {
        assert_eq!(encode_to_body(&ToBody::Interrupt).unwrap(), b"{\"t\":\"interrupt\",\"v\":1}\n".to_vec());
        assert_eq!(decode_to_body(b"{\"t\":\"interrupt\",\"v\":1}\n").unwrap(), ToBody::Interrupt);
    }
}
