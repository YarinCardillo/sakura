//! What the SDK's handlers and the session loop share: the open session, the permission requests that wait
//! for the person, what is known of each tool call (for the policy), and when the agent was last heard.

use super::AgentEvent;
use crate::convert;
use crate::model::{OptionKind, PermissionOption, ToolCallFields, Update};
use crate::policy::decider::{Decider, Verdict};
use crate::policy::{self, Decision, Reason, Runs, Subject};
use crate::thread::{PermissionOutcome, Problem, ProblemKind, ThreadEvent};
use crate::transport::{TakingIn, Unread};
use agent_client_protocol::v1::schema::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate,
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
    /// Which turn is the current one, counted at each opening and closing: a request is decided for the
    /// turn it was admitted in, and only while that turn is still the open one.
    turn: u64,
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
    /// Where the agent runs, for the policy.
    runs: Runs,
    /// Decides in place of the policy, when the agent has one (the engine, for its master).
    decider: Option<Decider>,
    /// The agent's lines the handlers have not dealt with yet.
    unread: Arc<Unread>,
    state: Mutex<State>,
}

impl Shared {
    pub(super) fn new(
        events: mpsc::Sender<AgentEvent>,
        workspace: PathBuf,
        runs: Runs,
        decider: Option<Decider>,
        unread: Arc<Unread>,
    ) -> Self {
        Self {
            events,
            workspace,
            runs,
            decider,
            unread,
            state: Mutex::new(State::new()),
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
        let SessionNotification {
            session_id, update, ..
        } = notification;
        // What the session offers is announced right after the answer that opens it, which the session
        // loop may not have recorded yet: it is heard from a session that is being opened too.
        if let Some(news) = session_news(&update) {
            if self.hears_or_opens(&session_id.0) {
                self.emit(news).await;
            }
            return;
        }
        // An update of a turn that timed out and has not confirmed its end is not shown: it would be
        // read into whatever the person does next.
        if !self.hears(&session_id.0) || self.state().is_settling {
            return;
        }
        let update = convert::update(update);
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
        let is_heard = self.hears(&request.session_id.0);
        let Some(turn) = self.state().admitting_turn().filter(|_| is_heard) else {
            return cancel(responder);
        };
        let wire = self.decider.as_ref().map(|_| wire_request(&request));
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
        let decision = match (&self.decider, wire) {
            (Some(decider), Some(wire)) => {
                let Some(wire) =
                    policy::master_command::acp_request(wire, &self.workspace, subject.kind)
                else {
                    return self.refuse(responder, &options, turn);
                };
                match decider.decide(wire).await {
                    Verdict::Deny => return self.refuse(responder, &options, turn),
                    verdict => allowed_or_asked(verdict, &options),
                }
            }
            _ => policy::decide_subject(&subject, &options, &self.workspace, self.runs),
        };
        // The turn may have been cancelled or ended while the request was decided: the decision is acted
        // on under the lock, only if that turn is still the open one.
        match decision {
            Decision::Allow { option_id } => {
                {
                    let state = self.state();
                    if !state.is_open_turn(turn) {
                        return cancel(responder);
                    }
                    select(responder, option_id);
                }
                self.emit_thread(ThreadEvent::PermissionAllowed { tool })
                    .await;
            }
            Decision::Ask(reason) => {
                let parked = self.state().park(responder, options.clone(), turn);
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

    /// Answers a request the decider denied, with its single-use "reject" (or its lasting one, or none
    /// when it offers no reject), without the person: only while the request's turn is still the open one.
    fn refuse(&self, responder: PermissionResponder, options: &[PermissionOption], turn: u64) {
        let state = self.state();
        let is_reject = |kind: OptionKind| options.iter().find(|option| option.kind == kind);
        let rejecting =
            is_reject(OptionKind::RejectOnce).or_else(|| is_reject(OptionKind::RejectAlways));
        match rejecting {
            Some(option) if state.is_open_turn(turn) => select(responder, option.id.clone()),
            _ => cancel(responder),
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
        self.state().open_turn();
    }

    /// The turn was cancelled or has ended: from now on no request is approved or parked, and every
    /// waiting one is answered "cancelled", as the protocol requires when a turn is cancelled.
    pub(super) async fn close_turn(&self) {
        let waiting = self.state().close_turn();
        for (request, waiting) in waiting {
            cancel(waiting.responder);
            let outcome = PermissionOutcome::Cancelled;
            self.emit_thread(ThreadEvent::PermissionAnswered { request, outcome })
                .await;
        }
    }

    /// Whether a message for `session` is for the open session or for one that is being opened.
    fn hears_or_opens(&self, session: &str) -> bool {
        let is_opening = self.state().session.is_none();
        is_opening || self.hears(session)
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
    fn new() -> Self {
        Self {
            session: None,
            is_turn_open: false,
            turn: 0,
            is_settling: false,
            waiting: HashMap::new(),
            next_request: 1,
            known: HashMap::new(),
            known_order: VecDeque::new(),
            last_heard: Instant::now(),
        }
    }

    /// Opens a turn and gives its number.
    fn open_turn(&mut self) -> u64 {
        self.turn += 1;
        self.is_turn_open = true;
        self.turn
    }

    /// Closes the turn and gives the requests that waited in it, to be answered "cancelled".
    fn close_turn(&mut self) -> Vec<(u64, Waiting)> {
        self.turn += 1;
        self.is_turn_open = false;
        self.waiting.drain().collect()
    }

    /// The turn a request arriving now is admitted in; none when no turn is open.
    fn admitting_turn(&self) -> Option<u64> {
        self.is_turn_open.then_some(self.turn)
    }

    /// Whether `turn` is still the open turn.
    fn is_open_turn(&self, turn: u64) -> bool {
        self.is_turn_open && self.turn == turn
    }

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
            name,
            status,
            content,
            locations,
            raw_input,
        } = fields.clone();
        known.title = title.or(known.title.take());
        known.kind = kind.or(known.kind);
        known.name = name.or(known.name.take());
        known.status = status.or(known.status);
        known.content = content.or(known.content.take());
        known.locations = locations.or(known.locations.take());
        known.raw_input = raw_input.or(known.raw_input.take());
    }

    /// Keeps a request admitted in `turn` for the person and returns its handle; `None` (answered
    /// "cancelled") when that turn is no longer the open one or too many already wait.
    fn park(
        &mut self,
        responder: PermissionResponder,
        options: Vec<PermissionOption>,
        turn: u64,
    ) -> Option<u64> {
        if !self.is_open_turn(turn) || self.waiting.len() >= MAX_WAITING_PERMISSIONS {
            cancel(responder);
            return None;
        }
        let request = self.next_request;
        self.next_request += 1;
        self.waiting.insert(request, Waiting { responder, options });
        Some(request)
    }
}

/// The request as the JSON-RPC message the agent sent, for the decider.
fn wire_request(request: &RequestPermissionRequest) -> serde_json::Value {
    let params = serde_json::to_value(request).unwrap_or(serde_json::Value::Null);
    serde_json::json!({ "method": "session/request_permission", "params": params })
}

/// What the confinement's decision becomes for a verdict that is not a denial: the single-use "allow" for
/// an allowed request (the person is asked when the agent offers none, as the policy does), the person
/// for the rest.
fn allowed_or_asked(verdict: Verdict, options: &[PermissionOption]) -> Decision {
    let allow_once = options
        .iter()
        .find(|option| option.kind == OptionKind::AllowOnce);
    match (verdict, allow_once) {
        (Verdict::Allow, Some(option)) => Decision::Allow {
            option_id: option.id.clone(),
        },
        (Verdict::Allow, None) => Decision::Ask(Reason::NoSingleUseOption),
        _ => Decision::Ask(Reason::EngineAsks),
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

/// What a session update says of the session itself rather than of the conversation: its commands, its
/// mode, its options.
fn session_news(update: &SessionUpdate) -> Option<AgentEvent> {
    Some(match update {
        SessionUpdate::AvailableCommandsUpdate(commands) => {
            AgentEvent::Commands(convert::commands(commands.clone()))
        }
        SessionUpdate::CurrentModeUpdate(mode) => {
            AgentEvent::ModeChanged(mode.current_mode_id.0.to_string())
        }
        SessionUpdate::ConfigOptionUpdate(options) => {
            let options = options.config_options.clone();
            AgentEvent::Choices(convert::choices(None, Some(options)))
        }
        _ => return None,
    })
}

pub(super) fn request_failed(method: &str, message: &str) -> Problem {
    Problem {
        kind: ProblemKind::RequestFailed,
        detail: format!("{method}: {message}"),
    }
}

pub(super) fn timed_out(method: &str) -> Problem {
    Problem {
        kind: ProblemKind::TimedOut,
        detail: method.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    /// A request is admitted in one turn and decided after: a cancel or the turn's end in between must
    /// leave its decision nothing to allow or park, also when a new turn has opened since.
    #[test]
    fn a_decision_for_a_turn_that_closed_meanwhile_is_stale() {
        let mut state = State::new();
        let turn = state.open_turn();
        assert!(state.is_open_turn(turn));
        let drained = state.close_turn();
        assert!(drained.is_empty());
        assert!(!state.is_open_turn(turn));
        let next = state.open_turn();
        assert!(
            !state.is_open_turn(turn),
            "a later turn is not the one admitted"
        );
        assert!(state.is_open_turn(next));
    }
}
