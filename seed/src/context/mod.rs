//! Context assembly (body/log-context.md §3.8–§3.14): the head and its hash, the projection from log
//! to model context, per-provider request layout (DeepSeek Chat Completions first), epochs, masking,
//! compaction with an anchored merged summary, self-signal placement and token estimation.

pub mod compact;
pub mod engine;
pub mod hash;
pub mod head;
pub mod mask;
pub mod model;
pub mod project;
pub mod render;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use engine::{
    AssistantIn, BoundaryIn, BoundaryOut, CallStart, Context, ContextError, NoticeIn, OwnerIn,
    ReloadIn, RequestOut, ResultIn, SignalSource,
};
#[allow(unused_imports)]
pub use head::{Head, build_head};
#[allow(unused_imports)]
pub use model::{Api, CacheProfile, Target, Thresholds};
