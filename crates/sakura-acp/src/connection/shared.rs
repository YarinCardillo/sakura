//! What the SDK's handlers and the session loop share: the open session, the permission requests that wait
//! for the person, what is known of each tool call (for the policy), and when the agent was last heard.

use super::AgentEvent;
use crate::convert;
use crate::model::{PermissionOption, ToolCallFields, Update};
use crate::policy::{self, Decision, Subject};
use crate::thread::{PermissionOutcome, ThreadEvent};
use crate::transport::{TakingIn, Unread};
use agent_client_protocol::v1::schema::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification,
};
use agent_client_protocol::v1::Responder;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Permission requests that may wait for the person at once; one more is answered "cancelled".
const MAX_WAITING_PERMISSIONS: usize = 32;
/// Tool calls whose fields are remembered for the policy; the oldest is forgotten first.
const MAX_KNOWN_CALLS: usize = 512;

type PermissionResponder = Responder<RequestPermissionResponse>;

/// A permission request the person has not answered.
struct Waiting {
    responder: PermissionResponder,
    options: Vec<PermissionOption>,
}

struct State {
    session: Option<String>,
    /// Whether a turn runs that was not cancelled: only then may the agent be given a permission.
    is_turn_open: bool,
    /// Whether a timed-out turn has not been confirmed stopped: what the agent sends is that turn's.
    is_settling: bool,
    waiting: HashMap<u64, Waiting>,
    next_request: u64,
    known: HashMap<String, ToolCallFields>,
    known_order: VecDeque<String>,
    last_heard: Instant,
}

/// Shared between the handlers and the session loop. The lock is never held across an await.
pub(super) struct Shared {
    events: mpsc::Sender<AgentEvent>,
    workspace: PathBuf,
    /// The agent's lines the handlers have not dealt with yet.
    unread: Arc<Unread>,
    state: Mutex<State>,
}

impl Shared {
    pub(super) fn new(
        events: mpsc::Sender<AgentEvent>,
        workspace: PathBuf,
        unread: Arc<Unread>,
    ) -> Self {
        let state = State {
            session: None,
            is_turn_open: false,
            is_settling: false,
            waiting: HashMap::new(),
            next_request: 1,
            known: HashMap::new(),
            known_order: VecDeque::new(),
            last_heard: Instant::now(),
        };
        Self {
            events,
            workspace,
            unread,
            state: Mutex::new(state),
        }
    }

    /// Sends an event to the window, waiting while its buffer is full. A window that is gone is not an
    /// error here: the session loop ends when its commands end.
    pub(super) async fn emit(&self, event: AgentEvent) {
        self.events.send(event).await.ok();
    }

    pub(super) async fn emit_thread(&self, event: ThreadEvent) {
        self.emit(AgentEvent::Thread(event)).await;
    }

    pub(super) fn session(&self) -> Option<String> {
        self.state().session.clone()
    }

    pub(super) fn set_session(&self, session: Option<String>) {
        self.state().session = session;
    }

    /// The agent was heard from, or the person answered it: the turn is not stalled.
    pub(super) fn touch(&self) {
        self.state().last_heard = Instant::now();
    }

    /// Whether the agent has been silent for `limit` with no question open (while one is, the person is
    /// the one who is thinking).
    pub(super) fn is_stalled(&self, limit: Duration) -> bool {
        let state = self.state();
        state.waiting.is_empty() && state.last_heard.elapsed() >= limit
    }

    /// A `session/update`: forwarded when it is for the open session and the conversation shows it.
    pub(super) async fn on_update(&self, notification: SessionNotification) {
        // The message is held until the window was given it (or it was dropped): then its room is free.
        let _taking_in = TakingIn::of(&self.unread);
        // An update of a turn that timed out and has not confirmed its end is not shown: it would be
        // read into whatever the person does next.
        if !self.hears(&notification.session_id.0) || self.state().is_settling {
            return;
        }
        let update = convert::update(notification.update);
        match &update {
            Update::Ignored => return,
            Update::ToolCall(fields) | Update::ToolCallUpdate(fields) => self.state().note(fields),
            _ => {}
        }
        self.emit_thread(ThreadEvent::Update(update)).await;
    }

    /// A `session/request_permission`: answered at once when the policy allows it, parked for the person
    /// otherwise, and refused when it is not for the open session, when two of its options share an id,
    /// or when too many already wait.
    pub(super) async fn on_permission(
        &self,
        request: RequestPermissionRequest,
        responder: PermissionResponder,
    ) {
        let _taking_in = TakingIn::of(&self.unread);
        // A request for another session, one that arrives after the turn was cancelled or ended, and
        // one sent with no turn running are all answered "cancelled": nothing is approved or asked.
        if !self.hears(&request.session_id.0) || !self.state().is_turn_open {
            return cancel(responder);
        }
        let tool = convert::tool_call_update(request.tool_call);
        let options = convert::options(request.options);
        if has_shared_id(&options) {
            // An answer names an option by its id alone: with two options under one id, "allow once"
            // could be taken for "allow always". Neither the policy nor the person answers such a request.
            eprintln!("sakura: a permission request offered two options under one id; refused");
            return cancel(responder);
        }
        // The subject is read once, from what is known of the call with the request's own fields on top;
        // the decision is made on it and the person is shown it.
        let subject = {
            let mut state = self.state();
            state.note(&tool);
            Subject::of(state.known.get(&tool.id).unwrap_or(&tool))
        };
        let decision = policy::decide_subject(&subject, &options, &self.workspace);
        match decision {
            Decision::Allow { option_id } => {
                select(responder, option_id);
                self.emit_thread(ThreadEvent::PermissionAllowed { tool })
                    .await;
            }
            Decision::Ask(reason) => {
                let parked = self.state().park(responder, options.clone());
                let Some(request) = parked else { return };
                let asked = ThreadEvent::PermissionAsked {
                    request,
                    tool,
                    reason,
                    subject,
                    options,
                };
                self.emit_thread(asked).await;
            }
        }
    }

    /// The person's answer to the request `request`. An option the request never offered is not sent.
    pub(super) async fn answer(&self, request: u64, option_id: &str) {
        let answered = {
            let mut state = self.state();
            state.last_heard = Instant::now();
            let chosen = state.waiting.get(&request).and_then(|waiting| {
                let option = waiting.options.iter().find(|option| option.id == option_id);
                option.map(|option| option.kind)
            });
            chosen.and_then(|kind| Some((state.waiting.remove(&request)?, kind)))
        };
        let Some((waiting, kind)) = answered else {
            return eprintln!("sakura: no waiting permission request {request} offers that option");
        };
        select(waiting.responder, option_id.to_string());
        let outcome = PermissionOutcome::Selected(kind);
        self.emit_thread(ThreadEvent::PermissionAnswered { request, outcome })
            .await;
    }

    /// A timed-out turn starts, or stops, waiting for the agent to confirm that it ended.
    pub(super) fn set_settling(&self, is_settling: bool) {
        self.state().is_settling = is_settling;
    }

    /// A prompt was sent: until the turn is cancelled or ends, the agent may ask for permissions.
    pub(super) fn open_turn(&self) {
        self.state().is_turn_open = true;
    }

    /// The turn was cancelled or has ended: from now on no request is approved or parked, and every
    /// waiting one is answered "cancelled", as the protocol requires when a turn is cancelled.
    pub(super) async fn close_turn(&self) {
        let waiting: Vec<(u64, Waiting)> = {
            let mut state = self.state();
            state.is_turn_open = false;
            state.waiting.drain().collect()
        };
        for (request, waiting) in waiting {
            cancel(waiting.responder);
            let outcome = PermissionOutcome::Cancelled;
            self.emit_thread(ThreadEvent::PermissionAnswered { request, outcome })
                .await;
        }
    }

    /// Whether a message for `session` is for the open session; when it is, the agent was heard from.
    fn hears(&self, session: &str) -> bool {
        let mut state = self.state();
        let is_open = state.session.as_deref() == Some(session);
        if is_open {
            state.last_heard = Instant::now();
        }
        is_open
    }

    /// The state, also after a panic elsewhere poisoned the lock: it holds plain data.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl State {
    /// Remembers the fields a message states for a tool call, on top of the ones already known.
    fn note(&mut self, fields: &ToolCallFields) {
        if !self.known.contains_key(&fields.id) {
            if self.known_order.len() >= MAX_KNOWN_CALLS {
                if let Some(oldest) = self.known_order.pop_front() {
                    self.known.remove(&oldest);
                }
            }
            self.known_order.push_back(fields.id.clone());
        }
        let known = self
            .known
            .entry(fields.id.clone())
            .or_insert_with(|| ToolCallFields::new(&fields.id));
        let ToolCallFields {
            id: _,
            title,
            kind,
            status,
            content,
            locations,
            raw_input,
        } = fields.clone();
        known.title = title.or(known.title.take());
        known.kind = kind.or(known.kind);
        known.status = status.or(known.status);
        known.content = content.or(known.content.take());
        known.locations = locations.or(known.locations.take());
        known.raw_input = raw_input.or(known.raw_input.take());
    }

    /// Keeps a request for the person and returns its handle; `None` (answered "cancelled") when too many
    /// already wait.
    fn park(
        &mut self,
        responder: PermissionResponder,
        options: Vec<PermissionOption>,
    ) -> Option<u64> {
        if self.waiting.len() >= MAX_WAITING_PERMISSIONS {
            cancel(responder);
            return None;
        }
        let request = self.next_request;
        self.next_request += 1;
        self.waiting.insert(request, Waiting { responder, options });
        Some(request)
    }
}

/// Whether two of `options` have the same id.
fn has_shared_id(options: &[PermissionOption]) -> bool {
    options
        .iter()
        .enumerate()
        .any(|(at, option)| options[..at].iter().any(|earlier| earlier.id == option.id))
}

fn select(responder: PermissionResponder, option_id: String) {
    let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id));
    respond(responder, outcome);
}

fn cancel(responder: PermissionResponder) {
    respond(responder, RequestPermissionOutcome::Cancelled);
}

/// A response that cannot be sent means the connection is closing; the session loop reports that.
fn respond(responder: PermissionResponder, outcome: RequestPermissionOutcome) {
    responder
        .respond(RequestPermissionResponse::new(outcome))
        .ok();
}
