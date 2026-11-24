//! Letting someone else decide a permission request: the engine for the master. The connection hands the
//! whole request, as the agent sent it, to the [`Decider`] and acts on one of three answers: the request
//! is allowed once without the person, refused without the person, or put to the person.

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// What a decider answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Answer with the request's single-use "allow", without asking the person.
    Allow,
    /// Answer with the request's single-use "reject", without the person ever seeing the request.
    Deny,
    /// Show the request to the person.
    Ask,
}

/// An answer on its way.
pub type Deciding = Pin<Box<dyn Future<Output = Verdict> + Send>>;

/// Decides each permission request of one agent. Cheap to clone; clones are the same decider.
#[derive(Clone)]
pub struct Decider(Arc<dyn Fn(Value) -> Deciding + Send + Sync>);

impl Decider {
    /// A decider that answers each request, given as the JSON-RPC message the agent sent
    /// (`{"method": "session/request_permission", "params": {...}}`), with `decide`.
    pub fn new(decide: impl Fn(Value) -> Deciding + Send + Sync + 'static) -> Self {
        Self(Arc::new(decide))
    }

    /// The verdict on `request`.
    pub async fn decide(&self, request: Value) -> Verdict {
        (self.0)(request).await
    }
}

impl std::fmt::Debug for Decider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Decider")
    }
}
