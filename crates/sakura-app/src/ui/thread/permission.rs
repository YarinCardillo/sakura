//! A permission request that reached the person: an inline card with what the agent asks and one button
//! per choice. The dot in the accent is here and nowhere else in the conversation, because this is the one
//! place that says "needs attention". Once answered it is a single quiet line.
//!
//! The card shows the request's subject as the confinement policy read it when the request arrived
//! (`sakura_acp::policy::Subject`): the same command text, the same paths, the same modes the decision was
//! made on, and the raw input as JSON when none of those say what the call is. It never shows the agent's
//! title as the subject, and it never reads the call's entry, which later messages can change.

use super::states::{self, Face};
use super::{copy, glyphs, rows, ThreadView};
use crate::l10n::tr;
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, text};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled,
};
use sakura_acp::model::{OptionKind, ToolKind, ToolStatus};
use sakura_acp::paths::{self, Place};
use sakura_acp::policy::Subject;
use sakura_acp::thread::{Permission, PermissionOutcome};
use std::path::Path;

/// One line of what a request is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SubjectLine {
    /// The command, whole.
    Command(String),
    /// A path inside the workspace, from the workspace.
    Inside(String),
    /// A path outside the workspace, whole and absolute.
    Outside(String),
    /// The mode a switch goes to.
    Mode(String),
    /// The host a fetch goes to, on a line of its own.
    Host(String),
    /// The whole address of a fetch.
    Address(String),
    /// A field of the raw input the policy does not read, as `name: value`.
    Field(String),
    /// The call's raw input as JSON: nothing else says what the call is.
    RawInput(String),
    /// The raw input goes on past what is shown.
    Cut,
    /// The request says nothing about itself.
    Nothing,
}

/// What the card says the request is about: every part of the subject the policy read, then the fields
/// of the raw input it did not read, or the raw input whole when no part says anything.
pub(super) fn subject_lines(subject: &Subject, workspace: &Path) -> Vec<SubjectLine> {
    let mut lines: Vec<SubjectLine> = subject
        .command
        .iter()
        .cloned()
        .map(SubjectLine::Command)
        .collect();
    lines.extend(
        subject
            .paths
            .iter()
            .map(|path| match paths::place(path, workspace) {
                Place::Inside(rest) if rest.is_empty() => SubjectLine::Inside(".".to_string()),
                Place::Inside(rest) => SubjectLine::Inside(rest),
                Place::Outside(whole) => SubjectLine::Outside(whole),
            }),
    );
    lines.extend(subject.modes.iter().cloned().map(SubjectLine::Mode));
    if let Some((host, address)) = fetched(subject) {
        // A fetch the card can read is shown as a person reads it: where it goes, then the address.
        lines.extend([SubjectLine::Host(host), SubjectLine::Address(address)]);
    } else if subject.is_unread() {
        lines.extend(subject.raw_input.iter().cloned().map(SubjectLine::RawInput));
        lines.extend(subject.is_raw_input_cut.then_some(SubjectLine::Cut));
    } else {
        // The raw input is not shown whole, so every field of it that the policy did not read is: a
        // working directory or a flag beside a harmless command is part of what is being asked.
        lines.extend(subject.unread.iter().cloned().map(SubjectLine::Field));
        lines.extend(subject.is_unread_cut.then_some(SubjectLine::Cut));
    }
    if lines.is_empty() {
        lines.push(SubjectLine::Nothing);
    }
    lines
}

/// The host and the address of a fetch whose raw input is one web address and nothing else: anything
/// more, anything cut, or an address that is not `http` or `https`, is shown as the raw input it is.
fn fetched(subject: &Subject) -> Option<(String, String)> {
    let is_fetch = subject.kind == Some(ToolKind::Fetch) && subject.is_unread();
    if !is_fetch || subject.is_raw_input_cut {
        return None;
    }
    let input: serde_json::Value = serde_json::from_str(subject.raw_input.as_deref()?).ok()?;
    let fields = input.as_object()?;
    let address = fields.get("url").filter(|_| fields.len() == 1)?.as_str()?;
    let host = sakura_core::links::web_host(address)?;
    Some((host, address.to_string()))
}

/// The state glyph of a settled request: allowed, refused or cancelled.
fn settled_status(outcome: PermissionOutcome) -> ToolStatus {
    match outcome {
        PermissionOutcome::Selected(kind) if kind.is_allow() => ToolStatus::Completed,
        PermissionOutcome::Selected(_) => ToolStatus::Rejected,
        PermissionOutcome::Cancelled | PermissionOutcome::Waiting => ToolStatus::Canceled,
    }
}

impl ThreadView {
    pub(super) fn permission(
        &self,
        permission: &Permission,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if permission.outcome != PermissionOutcome::Waiting {
            return rows::line_row()
                .child(glyphs::status(settled_status(permission.outcome), palette))
                .child(
                    text(theme::TEXT_SMALL, palette.text_tertiary)
                        .child(tr(copy::outcome_sentence(permission.outcome))),
                )
                .into_any_element();
        }
        let lines = subject_lines(&permission.subject, Path::new(&(self.host.workspace)(cx)));
        let question = div()
            .flex()
            .items_center()
            .gap(tokens::LINE_ROW_GAP)
            .child(glyphs::needs_you(palette))
            .child(
                text(theme::TEXT_BODY, palette.text)
                    .font_weight(theme::WEIGHT_MEDIUM)
                    .child(tr(copy::permission_question(permission.reason))),
            );
        div()
            .w_full()
            .p(tokens::CARD_PADDING)
            .flex()
            .flex_col()
            .gap(tokens::CARD_GAP)
            .debug_selector(|| "thread-permission-card".to_string())
            .rounded(theme::RADIUS_CARD)
            // A filled surface, the one the person's own message has: never an outline alone.
            .bg(palette.fill)
            .child(question)
            .child(details(&lines, palette))
            .child(self.choices(permission, palette, cx))
            .into_any_element()
    }

    /// One button per choice, in the order of what each one costs: allow once (the primary action),
    /// reject, reject always, and last and quietest allow always, because it widens what the agent may
    /// do later. A button keeps the name of its place in the agent's list.
    fn choices(
        &self,
        permission: &Permission,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request = permission.request;
        let mut ordered: Vec<(usize, &sakura_acp::model::PermissionOption)> =
            permission.options.iter().enumerate().collect();
        ordered.sort_by_key(|(_, option)| choice_rank(option.kind));
        let buttons = ordered.into_iter().map(|(at, option)| {
            let name = format!("thread-permission-{request}-{at}");
            let option_id = option.id.clone();
            states::button(
                SharedString::from(name.clone()),
                tr(copy::option_label(option.kind)),
                choice_face(option.kind),
                palette,
            )
            .debug_selector(move || name)
            .on_click(
                cx.listener(move |view, _, _, cx| view.answer(request, option_id.clone(), cx)),
            )
        });
        div()
            .flex()
            .flex_wrap()
            .gap(tokens::BUTTON_GAP)
            .children(buttons.collect::<Vec<_>>())
            .into_any_element()
    }
}

/// Where a choice stands among the card's buttons.
fn choice_rank(kind: OptionKind) -> u8 {
    match kind {
        OptionKind::AllowOnce => 0,
        OptionKind::RejectOnce => 1,
        OptionKind::RejectAlways => 2,
        OptionKind::AllowAlways => 3,
    }
}

/// How a choice is painted: allowing once is the action, allowing always the quietest.
fn choice_face(kind: OptionKind) -> Face {
    match kind {
        OptionKind::AllowOnce => Face::Primary,
        OptionKind::RejectOnce | OptionKind::RejectAlways => Face::OnFill,
        OptionKind::AllowAlways => Face::Quiet,
    }
}

/// The subject, whole and wrapped, never cut: the command and the raw input as they are, a path outside
/// the workspace marked as such.
fn details(lines: &[SubjectLine], palette: &Palette) -> AnyElement {
    let line = |line: &SubjectLine| match line {
        SubjectLine::Command(words)
        | SubjectLine::Inside(words)
        | SubjectLine::Field(words)
        | SubjectLine::RawInput(words) => div().child(copy::shown(words)),
        SubjectLine::Host(host) => div()
            .font_family(theme::FONT_UI)
            .text_size(theme::TEXT_SMALL)
            .font_weight(theme::WEIGHT_MEDIUM)
            .child(copy::shown(host)),
        SubjectLine::Address(address) => div().child(copy::shown(address)),
        SubjectLine::Outside(path) => div()
            .flex()
            .flex_wrap()
            .gap(tokens::LINE_ROW_GAP)
            .child(div().min_w_0().child(copy::shown(path)))
            .child(outside_mark(palette)),
        SubjectLine::Mode(mode) => div().child(format!("{} {}", tr(copy::MODE), copy::shown(mode))),
        SubjectLine::Cut => quiet(tr(copy::TRUNCATED), palette),
        SubjectLine::Nothing => quiet(tr(copy::NO_SUBJECT), palette),
    };
    mono()
        .w_full()
        .px(tokens::BLOCK_PADDING_X)
        .py(tokens::BLOCK_PADDING_Y)
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.inset)
        .text_size(theme::TEXT_LABEL)
        .line_height(gpui::px(theme::LOG_LINE_HEIGHT))
        .text_color(palette.text)
        .children(lines.iter().map(line))
        .into_any_element()
}

/// The words that mark a path as outside the workspace, in the warning tone.
pub(super) fn outside_mark(palette: &Palette) -> gpui::Div {
    text(theme::TEXT_LABEL, palette.warning)
        .font_family(theme::FONT_UI)
        .flex_none()
        .child(tr(copy::OUTSIDE))
}

fn quiet(words: &'static str, palette: &Palette) -> gpui::Div {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .font_family(theme::FONT_UI)
        .child(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_acp::model::{Location, ToolCallFields, ToolKind};
    use serde_json::json;

    const WORKSPACE: &str = "/work/demo";

    fn lines_of(tool: &ToolCallFields) -> Vec<SubjectLine> {
        subject_lines(&Subject::of(tool), Path::new(WORKSPACE))
    }

    fn tool(kind: ToolKind, input: serde_json::Value, paths: &[&str]) -> ToolCallFields {
        let location = |path: &&str| Location {
            path: path.to_string(),
            line: None,
        };
        ToolCallFields {
            kind: Some(kind),
            raw_input: Some(input),
            locations: Some(paths.iter().map(location).collect()),
            ..ToolCallFields::new("call")
        }
    }

    #[test]
    fn subject_lines_of_a_command_given_as_an_array_show_the_text_the_policy_lexes() {
        let tool = tool(
            ToolKind::Execute,
            json!({ "command": ["git", "push", "origin"] }),
            &[],
        );
        assert_eq!(
            lines_of(&tool),
            [SubjectLine::Command("git push origin".into())]
        );
    }

    #[test]
    fn subject_lines_show_the_unread_fields_beside_the_command_they_come_with() {
        let input = json!({ "command": "cat config", "cwd": "/etc" });
        assert_eq!(
            lines_of(&tool(ToolKind::Execute, input, &[])),
            [
                SubjectLine::Command("cat config".into()),
                SubjectLine::Field(r#"cwd: "/etc""#.into()),
            ]
        );
    }

    #[test]
    fn subject_lines_never_show_a_misleading_title() {
        let call = ToolCallFields {
            title: Some("Read /work/demo/README.md".into()),
            ..tool(ToolKind::Other, json!({ "to": "attacker" }), &[])
        };
        assert_eq!(
            lines_of(&call),
            [SubjectLine::RawInput(r#"{"to":"attacker"}"#.into())]
        );
        let silent = ToolCallFields {
            title: Some("Read /work/demo/README.md".into()),
            kind: Some(ToolKind::Other),
            ..ToolCallFields::new("call")
        };
        assert_eq!(lines_of(&silent), [SubjectLine::Nothing]);
    }

    #[test]
    fn subject_lines_show_paths_from_the_workspace_and_mark_the_ones_outside() {
        let paths = [
            "/work/demo/src/lib.rs",
            "/work/demo-evil/x",
            "/work/demo/../other/x",
            "/work/demo/a/../b.rs",
            "/work/demo",
        ];
        let tool = tool(ToolKind::Edit, json!({}), &paths);
        assert_eq!(
            lines_of(&tool),
            [
                SubjectLine::Inside("src/lib.rs".into()),
                SubjectLine::Outside("/work/demo-evil/x".into()),
                SubjectLine::Outside("/work/other/x".into()),
                SubjectLine::Inside("b.rs".into()),
                SubjectLine::Inside(".".into()),
            ]
        );
    }

    #[test]
    fn subject_lines_of_a_mode_switch_show_every_mode_it_names() {
        let tool = tool(
            ToolKind::SwitchMode,
            json!({ "mode": "plan", "modeId": "bypassPermissions" }),
            &[],
        );
        let modes = [
            SubjectLine::Mode("plan".into()),
            SubjectLine::Mode("bypassPermissions".into()),
        ];
        assert_eq!(lines_of(&tool), modes);
    }

    #[test]
    fn subject_lines_of_a_fetch_show_the_host_then_the_whole_address() {
        let url = tool(
            ToolKind::Fetch,
            json!({ "url": "https://crates.io/crates/serde?x=1" }),
            &[],
        );
        assert_eq!(
            lines_of(&url),
            [
                SubjectLine::Host("crates.io".into()),
                SubjectLine::Address("https://crates.io/crates/serde?x=1".into()),
            ]
        );
    }

    #[test]
    fn subject_lines_of_a_fetch_the_card_cannot_read_show_its_raw_input_and_say_when_it_is_cut() {
        // The fields are written in the order the JSON library keeps them, which depends on how it is
        // built: what counts is that both are shown.
        let asked = json!({ "url": "https://example.com", "method": "POST" });
        let more = tool(ToolKind::Fetch, asked.clone(), &[]);
        assert_eq!(
            lines_of(&more),
            [SubjectLine::RawInput(asked.to_string())],
            "a second field is part of what is asked: the whole input is shown"
        );
        let local = tool(ToolKind::Fetch, json!({ "url": "file:///etc/passwd" }), &[]);
        assert!(matches!(lines_of(&local)[0], SubjectLine::RawInput(_)));
        let long = tool(ToolKind::Fetch, json!({ "url": "x".repeat(8192) }), &[]);
        assert_eq!(lines_of(&long).last(), Some(&SubjectLine::Cut));
    }

    #[test]
    fn the_choices_stand_in_the_order_of_their_cost_with_allow_always_last_and_quietest() {
        let mut kinds = [
            OptionKind::AllowAlways,
            OptionKind::RejectOnce,
            OptionKind::AllowOnce,
            OptionKind::RejectAlways,
        ];
        kinds.sort_by_key(|kind| choice_rank(*kind));
        assert_eq!(
            kinds,
            [
                OptionKind::AllowOnce,
                OptionKind::RejectOnce,
                OptionKind::RejectAlways,
                OptionKind::AllowAlways,
            ]
        );
        assert!(choice_face(OptionKind::AllowOnce) == Face::Primary);
        assert!(
            choice_face(OptionKind::RejectOnce) == Face::OnFill,
            "a surface of its own on the card's fill"
        );
        assert!(choice_face(OptionKind::AllowAlways) == Face::Quiet);
    }

    #[test]
    fn settled_status_tells_allowed_refused_and_cancelled_apart() {
        let allowed = PermissionOutcome::Selected(OptionKind::AllowAlways);
        let refused = PermissionOutcome::Selected(OptionKind::RejectOnce);
        assert_eq!(settled_status(allowed), ToolStatus::Completed);
        assert_eq!(settled_status(refused), ToolStatus::Rejected);
        assert_eq!(
            settled_status(PermissionOutcome::Cancelled),
            ToolStatus::Canceled
        );
    }
}
