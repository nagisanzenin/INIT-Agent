//! `Client`: one adapter plus the transport. It implements the loop's `ModelClient` seam
//! (loop.md §3.2): the context-rendered body is sent unchanged with the API's path and headers.

use super::http::Cx;
use super::transport::{EventStream, Transport, stream};
use super::{Prompt, Provider, for_model};
use crate::model::{Abort, ModelClient, ModelInfo, ProviderError, Purpose, Request, StreamEvent};

pub struct Client {
    provider: Box<dyn Provider>,
    transport: Transport,
}

impl Client {
    pub fn new(info: &'static ModelInfo, transport: Transport) -> Self {
        Client {
            provider: for_model(info),
            transport,
        }
    }

    /// `INIT_PROXY_SOCK` and the `INIT_TEST_*` overrides.
    pub fn from_env(info: &'static ModelInfo) -> Result<Self, String> {
        Ok(Self::new(info, Transport::from_env()?))
    }

    pub fn with_provider(provider: Box<dyn Provider>, transport: Transport) -> Self {
        Client {
            provider,
            transport,
        }
    }

    pub fn provider(&self) -> &dyn Provider {
        self.provider.as_ref()
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Sends request body bytes (rendered by the context, or by `Provider::build`).
    pub fn send_body<'a>(&'a self, body: Vec<u8>, cx: Cx, abort: &'a Abort) -> EventStream<'a> {
        stream(
            self.provider.as_ref(),
            &self.transport,
            self.provider.wire(body),
            cx,
            abort,
        )
    }

    /// Builds a prompt with the adapter and sends it.
    pub fn send_prompt<'a>(
        &'a self,
        p: &Prompt,
        cx: Cx,
        abort: &'a Abort,
    ) -> Result<EventStream<'a>, ProviderError> {
        let req = self.provider.build(p)?;
        Ok(stream(
            self.provider.as_ref(),
            &self.transport,
            req,
            cx,
            abort,
        ))
    }
}

pub fn cx(session: &str, purpose: Purpose, task: Option<&str>, subtask: Option<&str>) -> Cx {
    Cx {
        session: session.to_string(),
        purpose: purpose.as_str(),
        task: task.map(str::to_string),
        subtask: subtask.map(str::to_string),
        caps: super::caps::take(),
    }
}

impl ModelClient for Client {
    fn info(&self) -> &ModelInfo {
        self.provider.info()
    }

    fn stream<'a>(
        &'a self,
        req: &'a Request<'a>,
        abort: &'a Abort,
    ) -> Box<dyn Iterator<Item = StreamEvent> + 'a> {
        Box::new(self.send_body(
            req.body.to_vec(),
            cx(req.session, req.purpose, req.task, req.subtask),
            abort,
        ))
    }
}
