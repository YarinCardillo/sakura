//! The permission requests the fake agent sent and that wait for the client's answer. A response is
//! matched to its request by the request's id, and what it means is read from the kind of the option
//! the request offered under the chosen id, as a real agent does; nothing is inferred from an id's
//! spelling.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

/// What the client answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allowed,
    Refused,
    /// The client answered "cancelled", as it does when the turn is cancelled.
    Cancelled,
}

struct Waiting {
    answers: Sender<Answer>,
    /// The kind of each option the request offered, by option id.
    kinds: HashMap<String, String>,
}

/// The requests that wait, by id.
#[derive(Default)]
pub struct Asked {
    waiting: Mutex<HashMap<u64, Waiting>>,
}

impl Asked {
    /// Notes the request `id`, which offered `options`, and returns where its answer will arrive.
    pub fn ask(&self, id: u64, options: &Value) -> Receiver<Answer> {
        let (answers, answered) = channel();
        let offered = options.as_array().into_iter().flatten();
        let kinds = offered
            .filter_map(|option| {
                let id = option.get("optionId")?.as_str()?;
                let kind = option.get("kind")?.as_str()?;
                Some((id.to_string(), kind.to_string()))
            })
            .collect();
        let mut waiting = self.waiting.lock().expect("the waiting requests");
        waiting.insert(id, Waiting { answers, kinds });
        answered
    }

    /// Hands the response `message` to the request it answers; false when it answers none that waits.
    pub fn answer(&self, message: &Value) -> bool {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return false;
        };
        let request = self
            .waiting
            .lock()
            .expect("the waiting requests")
            .remove(&id);
        let Some(request) = request else {
            return false;
        };
        let outcome = &message["result"]["outcome"];
        let chosen = outcome["optionId"].as_str().unwrap_or("");
        let kind = request.kinds.get(chosen).map(String::as_str);
        let answer = match (outcome["outcome"] == "selected", kind) {
            (true, Some(kind)) if kind.starts_with("allow") => Answer::Allowed,
            // A refusal, or an option the request never offered.
            (true, _) => Answer::Refused,
            (false, _) => Answer::Cancelled,
        };
        request.answers.send(answer).is_ok()
    }

    /// Stops waiting for the request `id`: its turn has given up on it.
    pub fn forget(&self, id: u64) {
        self.waiting
            .lock()
            .expect("the waiting requests")
            .remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options() -> Value {
        json!([
            { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
            { "optionId": "allow-nothing", "name": "Reject", "kind": "reject_once" },
        ])
    }

    fn selected(id: u64, option: &str) -> Value {
        let outcome = json!({ "outcome": "selected", "optionId": option });
        json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } })
    }

    #[test]
    fn a_response_answers_only_the_request_with_its_id() {
        let asked = Asked::default();
        let first = asked.ask(9000, &options());
        assert!(!asked.answer(&selected(7, "yes")), "no request 7 waits");
        assert!(first.try_recv().is_err(), "request 9000 still waits");
        assert!(asked.answer(&selected(9000, "yes")));
        assert_eq!(first.try_recv(), Ok(Answer::Allowed));
        assert!(
            !asked.answer(&selected(9000, "yes")),
            "it was answered once"
        );
    }

    #[test]
    fn two_requests_that_wait_are_each_answered_by_their_own_response() {
        let asked = Asked::default();
        let first = asked.ask(9000, &options());
        let second = asked.ask(9001, &options());
        assert!(asked.answer(&selected(9001, "allow-nothing")));
        assert!(first.try_recv().is_err());
        assert_eq!(second.try_recv(), Ok(Answer::Refused));
        assert!(asked.answer(&selected(9000, "yes")));
        assert_eq!(first.try_recv(), Ok(Answer::Allowed));
    }

    #[test]
    fn an_answer_means_what_the_kind_of_the_chosen_option_says_not_what_its_id_looks_like() {
        let asked = Asked::default();
        let allowed = asked.ask(1, &options());
        asked.answer(&selected(1, "yes"));
        assert_eq!(allowed.try_recv(), Ok(Answer::Allowed), "kind allow_once");
        let refused = asked.ask(2, &options());
        asked.answer(&selected(2, "allow-nothing"));
        assert_eq!(refused.try_recv(), Ok(Answer::Refused), "kind reject_once");
        let unknown = asked.ask(3, &options());
        asked.answer(&selected(3, "allow-never-offered"));
        assert_eq!(
            unknown.try_recv(),
            Ok(Answer::Refused),
            "an option it never offered"
        );
    }

    #[test]
    fn a_cancelled_outcome_is_its_own_answer() {
        let asked = Asked::default();
        let waiting = asked.ask(5, &options());
        let outcome = json!({ "outcome": "cancelled" });
        asked.answer(&json!({ "jsonrpc": "2.0", "id": 5, "result": { "outcome": outcome } }));
        assert_eq!(waiting.try_recv(), Ok(Answer::Cancelled));
    }
}
