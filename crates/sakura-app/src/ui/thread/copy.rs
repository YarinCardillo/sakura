//! The words of the conversation, in one place, as keys of the locale files. Text that comes from the
//! agent passes through [`shown`] before it is drawn.

use crate::l10n::{tr_with, Key};
use gpui::SharedString;
use sakura_acp::model::{OptionKind, PlanEntry, PlanStatus, ToolKind, ToolStatus};
use sakura_acp::policy::Reason;
use sakura_acp::thread::{PermissionOutcome, ProblemKind};
use std::time::Duration;

pub const PLACEHOLDER: Key = Key("thread.placeholder");
pub const WORKING: Key = Key("thread.working");
pub const STOPPING: Key = Key("thread.stopping");
pub const THINKING: Key = Key("thread.thinking");
pub const THOUGHT: Key = Key("thread.thought");
pub const PLAN: Key = Key("thread.plan");
pub const JUMP_TO_END: Key = Key("thread.jump_to_end");
pub const SEND: Key = Key("thread.send");
pub const STOP: Key = Key("thread.stop");
pub const RESTART: Key = Key("thread.restart");
pub const SIGN_IN: Key = Key("thread.sign_in");
pub const RETRY: Key = Key("common.retry");
pub const NEEDS_YOU: Key = Key("thread.needs_you");
pub const NO_SESSION: Key = Key("thread.no_session");
pub const CONNECTING: Key = Key("thread.connecting");
pub const PREPARING: Key = Key("thread.preparing");
pub const PREPARING_LINE: Key = Key("start.preparing");
pub const LOADING: Key = Key("thread.loading");
pub const SIGN_IN_NEEDED: Key = Key("thread.sign_in_needed");
pub const SIGN_IN_HINT: Key = Key("thread.sign_in_hint");
pub const EMPTY_READY: Key = Key("thread.ready");
pub const KEPT_LOST: Key = Key("thread.kept_lost");
pub const NO_CONVERSATION: Key = Key("thread.no_conversation");
pub const NO_ACCOUNT: Key = Key("thread.no_account");
pub const NO_ACCOUNT_AT_ALL: Key = Key("thread.no_account_at_all");
pub const OPEN_TERMINAL: Key = Key("thread.open_terminal");
pub const LOGIN_EXPIRED_HINT: Key = Key("thread.login_expired_hint");
pub const DIFF_TOO_LARGE: Key = Key("thread.diff_too_large");
pub const NO_CHANGE: Key = Key("thread.no_change");
pub const TRUNCATED: Key = Key("thread.truncated");
pub const NEW_FILE: Key = Key("thread.new_file");
pub const OUTSIDE: Key = Key("thread.outside");
pub const MODE: Key = Key("thread.mode");
pub const NO_SUBJECT: Key = Key("thread.no_subject");

/// What is said when the person's own login on this Mac no longer works: with the account's name when
/// it is known.
pub fn login_expired(account: Option<&str>) -> SharedString {
    match account {
        Some(account) => tr_with(Key("thread.login_expired"), &[("account", account)]),
        None => crate::l10n::tr(Key("thread.login_expired_unnamed")).into(),
    }
}

/// The word beside a tool call's state glyph.
pub fn status_word(status: ToolStatus) -> Key {
    match status {
        ToolStatus::Pending => Key("thread.status.pending"),
        ToolStatus::InProgress => Key("thread.status.running"),
        ToolStatus::Completed => Key("thread.status.done"),
        ToolStatus::Failed => Key("thread.status.failed"),
        // The card under the row is what says the person is needed; the row says what it waits for.
        ToolStatus::WaitingForConfirmation => Key("thread.status.waiting"),
        ToolStatus::Rejected => Key("thread.status.rejected"),
        ToolStatus::Canceled => Key("thread.status.cancelled"),
    }
}

/// What a tool call is called when the agent gave it no title.
pub fn kind_word(kind: ToolKind) -> Key {
    match kind {
        ToolKind::Read => Key("thread.kind.read"),
        ToolKind::Edit => Key("thread.kind.edit"),
        ToolKind::Delete => Key("thread.kind.delete"),
        ToolKind::Move => Key("thread.kind.move"),
        ToolKind::Search => Key("thread.kind.search"),
        ToolKind::Execute => Key("thread.kind.execute"),
        ToolKind::Think => Key("thread.thought"),
        ToolKind::Fetch => Key("thread.kind.fetch"),
        ToolKind::SwitchMode => Key("thread.kind.switch_mode"),
        ToolKind::Other => Key("thread.kind.other"),
    }
}

/// The card's sentence: what the agent asks for, by the reason the policy did not answer by itself.
pub fn permission_question(reason: Reason) -> Key {
    match reason {
        Reason::Network => Key("thread.question.network"),
        Reason::OutsideWorkspace => Key("thread.question.outside"),
        Reason::ModeChange => Key("thread.question.mode"),
        Reason::UnknownCommand => Key("thread.question.command"),
        Reason::UnknownKind => Key("thread.question.kind"),
        Reason::UnknownTarget => Key("thread.question.target"),
        Reason::UnknownField => Key("thread.question.field"),
        Reason::NoSingleUseOption => Key("thread.question.lasting"),
    }
}

/// A choice of the card, by what it does; the agent's own label is not used.
pub fn option_label(kind: OptionKind) -> Key {
    match kind {
        OptionKind::AllowOnce => Key("thread.option.allow_once"),
        OptionKind::AllowAlways => Key("thread.option.allow_always"),
        OptionKind::RejectOnce => Key("thread.option.reject_once"),
        OptionKind::RejectAlways => Key("thread.option.reject_always"),
    }
}

/// What a settled permission says in one line.
pub fn outcome_sentence(outcome: PermissionOutcome) -> Key {
    match outcome {
        PermissionOutcome::Waiting => NEEDS_YOU,
        PermissionOutcome::Selected(OptionKind::AllowOnce) => Key("thread.outcome.allowed_once"),
        PermissionOutcome::Selected(OptionKind::AllowAlways) => {
            Key("thread.outcome.allowed_always")
        }
        PermissionOutcome::Selected(OptionKind::RejectOnce) => Key("thread.outcome.rejected"),
        PermissionOutcome::Selected(OptionKind::RejectAlways) => {
            Key("thread.outcome.rejected_always")
        }
        PermissionOutcome::Cancelled => Key("thread.outcome.cancelled"),
    }
}

/// One sentence for why the conversation stopped.
pub fn problem_sentence(kind: ProblemKind) -> Key {
    match kind {
        ProblemKind::NotStarted => Key("thread.problem.not_started"),
        ProblemKind::Died => Key("thread.problem.died"),
        ProblemKind::AuthRequired => SIGN_IN_NEEDED,
        ProblemKind::Overrun => Key("thread.problem.overrun"),
        ProblemKind::Full => Key("thread.problem.full"),
        ProblemKind::TimedOut => Key("thread.problem.timed_out"),
        ProblemKind::RequestFailed => Key("thread.problem.request_failed"),
        ProblemKind::Refusal => Key("thread.problem.refusal"),
        ProblemKind::Stopped => Key("thread.problem.stopped"),
        ProblemKind::MaxTokens => Key("thread.problem.max_tokens"),
        ProblemKind::MaxTurnRequests => Key("thread.problem.max_requests"),
    }
}

/// How many steps of a plan are done, of how many.
pub fn plan_progress(entries: &[PlanEntry]) -> SharedString {
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanStatus::Completed)
        .count();
    let (done, total) = (done.to_string(), entries.len().to_string());
    tr_with(
        Key("thread.plan.progress"),
        &[("done", &done), ("total", &total)],
    )
}

/// What opens the rest of a folded block, with how many lines it has.
pub fn show_all_lines(lines: usize) -> SharedString {
    tr_with(
        Key("thread.show_all_lines"),
        &[("lines", &lines.to_string())],
    )
}

/// How long a turn has taken: "8 s", "2 min 05 s".
pub fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds / 60 {
        0 => format!("{seconds} s"),
        minutes => format!("{minutes} min {:02} s", seconds % 60),
    }
}

/// Text from the agent as it is shown: control characters (but not line breaks and tabs), bidirectional
/// controls and zero-width characters become a visible mark, so a line cannot read differently from what
/// it is. The same set as the review list's (`changes::line`), kept per character so a streamed chunk can
/// be cleaned without knowing its neighbours.
pub fn shown(text: &str) -> String {
    text.chars().map(shown_char).collect()
}

fn shown_char(c: char) -> char {
    let is_hidden = matches!(
        c,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    );
    let is_control = c.is_control() && c != '\n' && c != '\t';
    if is_hidden || is_control {
        char::REPLACEMENT_CHARACTER
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_under_a_minute_is_seconds_and_over_it_minutes_and_two_digit_seconds() {
        assert_eq!(elapsed(Duration::from_millis(900)), "0 s");
        assert_eq!(elapsed(Duration::from_secs(8)), "8 s");
        assert_eq!(elapsed(Duration::from_secs(125)), "2 min 05 s");
    }

    #[test]
    fn shown_keeps_line_breaks_and_marks_control_and_invisible_characters() {
        assert_eq!(shown("a\n\tb"), "a\n\tb");
        assert_eq!(shown("a\u{1b}[31mb\u{0}"), "a\u{fffd}[31mb\u{fffd}");
        assert_eq!(shown("admin\u{202e}x\u{200b}"), "admin\u{fffd}x\u{fffd}");
        assert_eq!(shown("cafés"), "cafés");
    }

    #[test]
    fn plan_progress_counts_the_completed_steps() {
        let entry = |status| PlanEntry {
            content: String::new(),
            status,
        };
        let entries = [
            entry(PlanStatus::Completed),
            entry(PlanStatus::InProgress),
            entry(PlanStatus::Pending),
        ];
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(plan_progress(&entries).as_ref(), "1 di 3");
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(plan_progress(&entries).as_ref(), "1 of 3");
    }

    #[test]
    fn every_status_kind_reason_and_problem_has_its_own_words() {
        use std::collections::HashSet;
        let statuses = [
            ToolStatus::Pending,
            ToolStatus::InProgress,
            ToolStatus::Completed,
            ToolStatus::Failed,
            ToolStatus::WaitingForConfirmation,
            ToolStatus::Rejected,
            ToolStatus::Canceled,
        ];
        let words: HashSet<Key> = statuses.iter().map(|status| status_word(*status)).collect();
        assert_eq!(words.len(), statuses.len());
        let reasons = [
            Reason::Network,
            Reason::OutsideWorkspace,
            Reason::ModeChange,
            Reason::UnknownCommand,
            Reason::UnknownKind,
            Reason::UnknownTarget,
            Reason::UnknownField,
            Reason::NoSingleUseOption,
        ];
        let questions: HashSet<Key> = reasons.iter().map(|r| permission_question(*r)).collect();
        assert_eq!(questions.len(), reasons.len());
    }
}
