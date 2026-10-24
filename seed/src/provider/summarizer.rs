//! The compaction summarizer (log-context.md §3.12.3; WP12's `compact::Summarizer`): sends the
//! context-rendered fork request with `x-init-purpose: compaction` and maps the reply.

use std::sync::{Arc, Mutex};

use super::client::Client;
use super::http::Cx;
use crate::context::compact::{ForkRequest, Summarizer, SummaryReply};
use crate::model::{Abort, AssistantMessage, ContentBlock, Purpose, StopReason, StreamEvent};

/// The session and task headers the summarizer sends. The loop updates them (`/new`, a new run);
/// the purpose is always `compaction`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SummaryCx {
    pub session: String,
    pub task: Option<String>,
}

pub struct ProxySummarizer {
    client: Arc<Client>,
    abort: Arc<Abort>,
    cx: Arc<Mutex<SummaryCx>>,
}

impl ProxySummarizer {
    pub fn new(client: Arc<Client>, abort: Arc<Abort>, cx: Arc<Mutex<SummaryCx>>) -> Self {
        ProxySummarizer { client, abort, cx }
    }
}

/// Maps a completed reply to the context's `SummaryReply` (DeepSeek reasoning kept for the retry).
pub fn reply(m: &AssistantMessage) -> SummaryReply {
    let thinking: String = m
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Thinking(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    SummaryReply {
        text: m.text(),
        thinking: (!thinking.is_empty()).then_some(thinking),
        tool_use: m.tool_calls().next().is_some(),
        model: m.origin.model.clone(),
        usage: Some(m.usage.to_log(None)),
    }
}

impl Summarizer for ProxySummarizer {
    fn summarize(&mut self, req: &ForkRequest) -> Result<SummaryReply, String> {
        let c = self.cx.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let cx = Cx {
            session: c.session,
            purpose: Purpose::Compaction.as_str(),
            task: c.task,
            subtask: None,
            // A compaction leaves the marks to the next turn request it serves.
            caps: None,
        };
        for ev in self.client.send_body(req.body.to_vec(), cx, &self.abort) {
            match ev {
                StreamEvent::Done(m) if m.stop == StopReason::Aborted => {
                    return Err("aborted".into());
                }
                StreamEvent::Done(m) => return Ok(reply(&m)),
                StreamEvent::Failed(e) => return Err(e.to_string()),
                _ => {}
            }
        }
        Err("stream ended without a terminal event".into())
    }
}
