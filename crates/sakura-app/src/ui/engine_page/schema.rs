//! The pipeline of a lane, drawn: Start, Plan, Build, Checks and Deliver in a row, a review above Plan and
//! above Build with its loop, a final review on the way from Build to Checks, and the guard as a band under
//! every step. What is on and what runs each step is read from the engine's rules; with none, the drawing
//! shows the standard pipeline, read only.

use super::words::{account_words, model_words};
use crate::l10n::{tr, Key};
use crate::theme::engine_page as sizes;
use crate::theme::kit::scale::{Typeset as _, TEXT_BODY, TEXT_CAPTION, TEXT_FOOTNOTE};
use crate::theme::{self, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    canvas, div, point, px, Animation, AnimationExt as _, AnyElement, App, Div, Hsla,
    InteractiveElement as _, IntoElement as _, ParentElement as _, PathBuilder, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use sakura_core::engine::rules::Rules;
use sakura_core::engine::snapshot::capitalised;
use std::rc::Rc;

/// A step, a review or a fixed part of the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    Start,
    Plan,
    PlanReview,
    Build,
    TaskReview,
    FinalReview,
    Checks,
    Deliver,
    Guard,
}

impl Node {
    /// Every node, in the order they are drawn.
    pub const ALL: [Node; 9] = [
        Node::Start,
        Node::Plan,
        Node::PlanReview,
        Node::Build,
        Node::TaskReview,
        Node::FinalReview,
        Node::Checks,
        Node::Deliver,
        Node::Guard,
    ];

    /// What names the node for the engine's rules and in the window.
    pub fn id(self) -> &'static str {
        match self {
            Node::Start => "start",
            Node::Plan => "plan",
            Node::PlanReview => "plan_review",
            Node::Build => "build",
            Node::TaskReview => "task_review",
            Node::FinalReview => "final_review",
            Node::Checks => "checks",
            Node::Deliver => "deliver",
            Node::Guard => "guard",
        }
    }

    /// The node's name on the drawing and above its rules.
    pub fn label(self) -> Key {
        match self {
            Node::Start => Key("engine_page.node.start"),
            Node::Plan => Key("engine_page.node.plan"),
            Node::PlanReview => Key("engine_page.node.plan_review"),
            Node::Build => Key("engine_page.node.build"),
            Node::TaskReview => Key("engine_page.node.task_review"),
            Node::FinalReview => Key("engine_page.node.final_review"),
            Node::Checks => Key("engine_page.node.checks"),
            Node::Deliver => Key("engine_page.node.deliver"),
            Node::Guard => Key("engine_page.node.guard"),
        }
    }

    /// What the node does, said above its rules.
    pub fn note(self) -> Key {
        match self {
            Node::Start => Key("engine_page.note.start"),
            Node::Plan => Key("engine_page.note.plan"),
            Node::PlanReview => Key("engine_page.note.plan_review"),
            Node::Build => Key("engine_page.note.build"),
            Node::TaskReview => Key("engine_page.note.task_review"),
            Node::FinalReview => Key("engine_page.note.final_review"),
            Node::Checks => Key("engine_page.note.checks"),
            Node::Deliver => Key("engine_page.note.deliver"),
            Node::Guard => Key("engine_page.note.guard"),
        }
    }

    fn rect(self) -> [f32; 4] {
        match self {
            Node::Start => sizes::START,
            Node::Plan => sizes::PLAN,
            Node::PlanReview => sizes::PLAN_REVIEW,
            Node::Build => sizes::BUILD,
            Node::TaskReview => sizes::TASK_REVIEW,
            Node::FinalReview => sizes::FINAL_REVIEW,
            Node::Checks => sizes::CHECKS,
            Node::Deliver => sizes::DELIVER,
            Node::Guard => sizes::GUARD,
        }
    }

    fn is_review(self) -> bool {
        matches!(
            self,
            Node::PlanReview | Node::TaskReview | Node::FinalReview
        )
    }

    /// The rule that turns the node on and off; none for a node that is always on.
    pub fn on_rule(self) -> Option<&'static str> {
        match self {
            Node::Plan => Some("steps.plan.on"),
            Node::PlanReview => Some("reviews.plan_review.on"),
            Node::TaskReview => Some("reviews.task_review.on"),
            Node::FinalReview => Some("reviews.final_review.on"),
            Node::Checks => Some("steps.checks.on"),
            _ => None,
        }
    }

    /// The rule that names the model the node runs on.
    pub(super) fn model_rule(self) -> Option<&'static str> {
        match self {
            Node::Plan => Some("steps.plan.model"),
            Node::PlanReview => Some("reviews.plan_review.model"),
            Node::Build => Some("steps.build.model"),
            Node::TaskReview => Some("reviews.task_review.model"),
            Node::FinalReview => Some("reviews.final_review.model"),
            _ => None,
        }
    }

    fn account_rule(self) -> Option<&'static str> {
        match self {
            Node::Plan => Some("steps.plan.account"),
            Node::PlanReview => Some("reviews.plan_review.account"),
            Node::Build => Some("steps.build.account"),
            Node::TaskReview => Some("reviews.task_review.account"),
            Node::FinalReview => Some("reviews.final_review.account"),
            _ => None,
        }
    }
}

/// The origin the engine gives a rule that the person changed: their own layer above the preset.
const EDITED_ORIGIN: &str = "rules";

/// What the drawing says of each node: whether it is on, its model and its account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    rules: Option<Rules>,
}

impl Pipeline {
    /// The pipeline of `rules`; with none, the standard one with no model named.
    pub fn from_rules(rules: Option<&Rules>) -> Self {
        Self {
            rules: rules.cloned(),
        }
    }

    /// Whether the node runs. A review of the plan needs the plan; the engine's own default for a node it
    /// does not say is that it is on, but for the final review, which is off.
    pub fn is_on(&self, node: Node) -> bool {
        if node == Node::PlanReview && !self.is_on(Node::Plan) {
            return false;
        }
        let Some(rule) = node.on_rule() else {
            return true;
        };
        match self.rules.as_ref().and_then(|rules| rules.rule(rule)) {
            Some(rule) => rule.as_bool().unwrap_or(true),
            None => node != Node::FinalReview,
        }
    }

    /// Whether a rule of the node comes from the person's own layer: it differs from the preset.
    pub fn is_edited(&self, node: Node) -> bool {
        let Some(rules) = self.rules.as_ref().and_then(|rules| rules.node(node.id())) else {
            return false;
        };
        rules.rules.iter().any(|rule| rule.origin == EDITED_ORIGIN)
    }

    /// What the node runs on in words, "Claude, Sonnet 5.5 high": the account (the program's name when the step
    /// runs on the project's), the model as its program lists it and the effort; none from an engine that does not
    /// say which agent a step runs on.
    pub fn agent_line(&self, node: Node) -> Option<SharedString> {
        let (account, model, effort) = self.agent_lines(node)?;
        Some(format!("{account}, {model}{effort}").into())
    }

    fn agent_lines(&self, node: Node) -> Option<(String, String, String)> {
        let rules = self.rules.as_ref()?;
        let (rule, agent) = self.rule_agent(node)?;
        let own_account = (node == Node::FinalReview)
            .then(|| rules.rule(node.account_rule()?).map(|rule| rule.as_text()))
            .flatten();
        let account_id = own_account.as_deref().unwrap_or(&agent.account);
        let account = match account_id {
            "project" | "" => capitalised(&agent.provider),
            // An account the rules do not name is the program's name, never its id.
            id => rules.account_label(id).map_or_else(
                || match agent.account_label.is_empty() {
                    false => agent.account_label.clone(),
                    true => capitalised(&agent.provider),
                },
                str::to_string,
            ),
        };
        let value = rule.as_text();
        let model = if agent.provider == "goose" {
            let mut accounts = rules.agents.iter().filter(|entry| {
                entry.provider == "goose" && (entry.id == account_id || account_id == "project")
            });
            let picked = accounts.next().filter(|_| accounts.next().is_none());
            picked
                .and_then(|entry| entry.models.iter().find(|choice| choice.id == agent.model))
                .map_or_else(
                    || tr(Key("goose.default_model")).to_string(),
                    |choice| choice.label.clone(),
                )
        } else {
            rules
                .model_label(&value)
                .map_or_else(|| model_words(&value).to_string(), str::to_string)
        };
        let effort = match agent.effort.is_empty() {
            true => String::new(),
            false => format!(
                " {}",
                crate::ui::master_agent::effort_words(&agent.effort).to_lowercase()
            ),
        };
        Some((account, model, effort))
    }

    /// The engine's words for a step that runs on an account of another provider than its own; none otherwise.
    pub fn agent_note(&self, node: Node) -> Option<SharedString> {
        let (_, agent) = self.rule_agent(node)?;
        Some(SharedString::from(agent.note.clone())).filter(|note| !note.is_empty())
    }

    /// The model rule of the node and the agent it says the step runs on. The engine has no model rule for the
    /// final review: it runs on the code reviewer's model, as the review of each task does.
    fn rule_agent(
        &self,
        node: Node,
    ) -> Option<(
        &sakura_core::engine::rules::Rule,
        &sakura_core::engine::rules::RuleAgent,
    )> {
        let rules = self.rules.as_ref()?;
        let rule = rules.rule(node.model_rule()?).or_else(|| {
            let shared = (node == Node::FinalReview).then_some(Node::TaskReview)?;
            rules.rule(shared.model_rule()?)
        })?;
        Some((rule, rule.agent.as_ref()?))
    }

    /// The model the node runs on, when the rules name it.
    pub fn model(&self, node: Node) -> Option<String> {
        let rule = self.rules.as_ref()?.rule(node.model_rule()?)?;
        Some(rule.as_text())
    }

    /// The account the node runs on, when the person has more than one and the rules name it.
    pub fn account(&self, node: Node) -> Option<String> {
        let rules = self.rules.as_ref()?;
        if rules.accounts_count < 2 {
            return None;
        }
        Some(rules.rule(node.account_rule()?)?.as_text())
    }
}

/// One line of the drawing: its points (two for a straight one, four for a curve), whether it is dashed and
/// whether it ends in an arrow.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub points: Vec<(f32, f32)>,
    pub is_dashed: bool,
    pub has_arrow: bool,
}

/// The edges for `pipeline`: the arrows between the steps, bent where a step is off, the reviews' loops
/// (dashed when the review is off) and the guard's dashed ties.
pub fn edges(pipeline: &Pipeline) -> Vec<Edge> {
    let arrow = |points: &[(f32, f32)], is_dashed: bool| Edge {
        points: points.to_vec(),
        is_dashed,
        has_arrow: true,
    };
    let mut found = Vec::new();
    if pipeline.is_on(Node::Plan) {
        found.push(arrow(&sizes::START_TO_PLAN, false));
        found.push(arrow(&sizes::PLAN_TO_BUILD, false));
    } else {
        found.push(arrow(&sizes::START_BYPASS, false));
    }
    if pipeline.is_on(Node::FinalReview) {
        found.push(arrow(&sizes::BUILD_TO_FINAL, false));
        found.push(arrow(&sizes::FINAL_TO_CHECKS, false));
    } else {
        found.push(arrow(&sizes::BUILD_TO_CHECKS, false));
    }
    found.push(arrow(&sizes::CHECKS_TO_DELIVER, false));
    let plan_off = !pipeline.is_on(Node::PlanReview);
    let task_off = !pipeline.is_on(Node::TaskReview);
    found.push(arrow(&sizes::PLAN_REVIEW_UP, plan_off));
    found.push(arrow(&sizes::PLAN_REVIEW_DOWN, plan_off));
    found.push(arrow(&sizes::TASK_REVIEW_UP, task_off));
    found.push(arrow(&sizes::TASK_REVIEW_DOWN, task_off));
    for tie in sizes::GUARD_TIES {
        found.push(Edge {
            points: tie.to_vec(),
            is_dashed: true,
            has_arrow: false,
        });
    }
    found
}

/// How many times each node has been switched on or off while the page was open: a node that has flipped is
/// drawn fading from its old look to its new, and starts again with each flip.
pub type Flips = std::collections::HashMap<Node, u32>;

/// What a press on a node does.
pub type OnSelect = Rc<dyn Fn(Node, &mut Window, &mut App)>;

/// The drawing: the edges under, the nodes over, `selected` filled. A press on a node calls `on_select`; with
/// `is_read_only` the nodes show and do nothing.
pub fn drawing(
    pipeline: &Pipeline,
    (selected, flips): (Node, &Flips),
    on_select: Option<OnSelect>,
    palette: &Palette,
) -> AnyElement {
    let lines = edges(pipeline);
    let ink = palette.text_tertiary;
    let canvas = canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            for edge in &lines {
                paint_edge(edge, bounds.origin, ink, window);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    let nodes = Node::ALL.map(|node| {
        let flip = flips.get(&node).copied().unwrap_or_default();
        node_view(
            node,
            pipeline,
            (node == selected, flip),
            on_select.clone(),
            palette,
        )
    });
    div()
        .debug_selector(|| "engine-schema".into())
        .relative()
        .flex_none()
        .w(sizes::SCHEMA_WIDTH)
        .h(sizes::SCHEMA_HEIGHT)
        .child(canvas)
        .children(nodes)
        .into_any_element()
}

fn node_view(
    node: Node,
    pipeline: &Pipeline,
    (is_selected, flip): (bool, u32),
    on_select: Option<OnSelect>,
    palette: &Palette,
) -> AnyElement {
    let [left, top, width, height] = node.rect();
    let is_on = pipeline.is_on(node);
    let NodeLook { face, ink, quiet } = colours(is_on, is_selected, palette);
    let selector = format!("engine-node-{}", node.id());
    let text_selector = format!("{selector}-text");
    let label_text = if node.is_review() {
        TEXT_FOOTNOTE
    } else {
        TEXT_BODY
    };
    let text = div()
        .debug_selector(move || text_selector.clone())
        .flex()
        .flex_col()
        .gap_0()
        .when(node == Node::Guard, |text| {
            text.flex_row().items_center().gap(theme::GAP_WIDE)
        })
        .child(
            div()
                .text_style(label_text)
                .font_weight(theme::WEIGHT_MEDIUM)
                .whitespace_nowrap()
                .child(short_label(node)),
        )
        .children(sub_lines(node, pipeline, is_on, quiet));
    let base = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .p(sizes::NODE_PADDING)
        .flex()
        .flex_col()
        .justify_center()
        .rounded(sizes::NODE_RADIUS)
        .bg(face)
        .text_color(ink)
        .child(text)
        .children(pipeline.is_edited(node).then(|| edited_mark(node, quiet)));
    let base = match on_select {
        Some(on_select) => base.on_click(move |_, window, cx| on_select(node, window, cx)),
        None => base,
    };
    match flip {
        0 => base.into_any_element(),
        flip => base
            .with_animation(
                SharedString::from(format!("engine-node-{}-flip-{flip}", node.id())),
                Animation::new(theme::DURATION_BASE).with_easing(crate::ui::slide::ease_out),
                |base, progress| {
                    base.opacity(sizes::NODE_FLIP_FROM + (1. - sizes::NODE_FLIP_FROM) * progress)
                },
            )
            .into_any_element(),
    }
}

/// The discreet mark of a step whose rules differ from the preset's: a small dot at the box's corner.
fn edited_mark(node: Node, ink: Hsla) -> Div {
    let selector = format!("engine-node-{}-edited", node.id());
    div()
        .debug_selector(move || selector.clone())
        .absolute()
        .top(sizes::NODE_MARK_INSET)
        .right(sizes::NODE_MARK_INSET)
        .size(sizes::NODE_MARK_SIZE)
        .rounded_full()
        .bg(ink)
}

/// What a node is drawn with: its fill (none for a step that is off), the outline an off step is drawn
/// with, the ink of its name and the ink of its small lines.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NodeLook {
    face: Hsla,
    ink: Hsla,
    quiet: Hsla,
}

fn colours(is_on: bool, is_selected: bool, palette: &Palette) -> NodeLook {
    let (face, ink) = if is_selected {
        (palette.selected_row, palette.text)
    } else if is_on {
        (palette.fill, palette.text)
    } else {
        (palette.inset, palette.text_tertiary)
    };
    NodeLook {
        face,
        ink,
        quiet: palette.text_tertiary,
    }
}

/// The node's name on the drawing: the reviews are only "Review" but the final one.
fn short_label(node: Node) -> SharedString {
    match node {
        Node::PlanReview | Node::TaskReview => tr(Key("engine_page.node.review")).into(),
        other => tr(other.label()).into(),
    }
}

/// The small lines under a node's name: what runs it, and the account when the person has several.
fn sub_lines(node: Node, pipeline: &Pipeline, is_on: bool, ink: Hsla) -> Vec<Div> {
    let whole = |words: SharedString| {
        div()
            .text_style(TEXT_CAPTION)
            .text_color(ink)
            .whitespace_nowrap()
            .child(words)
    };
    let line = |words: SharedString| whole(words).overflow_hidden().text_ellipsis();
    if node.on_rule().is_some() && !is_on {
        let off = line(tr(Key("engine_page.off")).into());
        let selector = format!("engine-node-{}-off", node.id());
        return vec![off.debug_selector(move || selector.clone())];
    }
    let fixed = match node {
        Node::Start => Some(Key("engine_page.run.start")),
        Node::Checks => Some(Key("engine_page.run.checks")),
        Node::Deliver => Some(Key("engine_page.run.deliver")),
        Node::Guard => Some(Key("engine_page.run.guard")),
        _ => None,
    };
    if let Some(words) = fixed {
        return vec![line(tr(words).into())];
    }
    if let Some((account, model, effort)) = pipeline.agent_lines(node) {
        let account_selector = format!("engine-node-{}-account", node.id());
        let model_selector = format!("engine-node-{}-model", node.id());
        let mut lines = vec![
            line(format!("{account}{effort}").into())
                .debug_selector(move || account_selector.clone()),
            whole(model.into())
                .whitespace_normal()
                .debug_selector(move || model_selector.clone()),
        ];
        lines.extend(pipeline.agent_note(node).map(|note| {
            let selector = format!("engine-node-{}-note", node.id());
            line(note).debug_selector(move || selector.clone())
        }));
        return lines;
    }
    // A model's name is short and is never cut; an account's may be long and is.
    let mut lines: Vec<Div> = pipeline
        .model(node)
        .map(|model| model_words(&model))
        .map(|words| {
            let selector = format!("engine-node-{}-model", node.id());
            whole(words).debug_selector(move || selector.clone())
        })
        .into_iter()
        .collect();
    if !node.is_review() {
        let account = pipeline.account(node).map(|account| {
            let selector = format!("engine-node-{}-account", node.id());
            line(account_words(&account)).debug_selector(move || selector.clone())
        });
        lines.extend(account);
    }
    lines
}

fn paint_edge(edge: &Edge, origin: gpui::Point<Pixels>, ink: Hsla, window: &mut Window) {
    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    let ink = if edge.is_dashed && edge.has_arrow {
        ink.opacity(sizes::EDGE_OFF_OPACITY)
    } else {
        ink
    };
    let mut stroke = PathBuilder::stroke(sizes::EDGE_WIDTH);
    if edge.is_dashed {
        stroke = stroke.dash_array(&[sizes::EDGE_DASH, sizes::EDGE_GAP]);
    }
    let points = &edge.points;
    stroke.move_to(at(points[0]));
    match points.len() {
        4 => stroke.cubic_bezier_to(at(points[3]), at(points[1]), at(points[2])),
        _ => stroke.line_to(at(points[1])),
    }
    if let Ok(path) = stroke.build() {
        window.paint_path(path, ink);
    }
    if edge.has_arrow {
        paint_arrowhead(points, origin, ink, window);
    }
}

/// A small filled triangle at the end of an edge, pointing along its last stretch.
fn paint_arrowhead(
    points: &[(f32, f32)],
    origin: gpui::Point<Pixels>,
    ink: Hsla,
    window: &mut Window,
) {
    let (tail, head) = (points[points.len() - 2], points[points.len() - 1]);
    let (dx, dy) = (head.0 - tail.0, head.1 - tail.1);
    let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (ux, uy) = (dx / length, dy / length);
    let base = (
        head.0 - ux * sizes::ARROW_LENGTH,
        head.1 - uy * sizes::ARROW_LENGTH,
    );
    let side = (-uy * sizes::ARROW_HALF_WIDTH, ux * sizes::ARROW_HALF_WIDTH);
    let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    let mut fill = PathBuilder::fill();
    fill.add_polygon(
        &[
            at(head),
            at((base.0 + side.0, base.1 + side.1)),
            at((base.0 - side.0, base.1 - side.1)),
        ],
        true,
    );
    if let Ok(path) = fill.build() {
        window.paint_path(path, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_on_and_selected_steps_use_three_distinct_surface_fills() {
        for palette in [Palette::light(), Palette::dark()] {
            let off = colours(false, false, &palette);
            assert_eq!(off.face, palette.inset);
            assert_eq!(off.ink, palette.text_tertiary);
            assert_eq!(colours(true, false, &palette).face, palette.fill);
            assert_eq!(colours(false, true, &palette).face, palette.selected_row);
        }
    }

    /// The words of an off step on the card it stands on are still readable text, in both appearances.
    #[test]
    fn the_words_of_a_step_that_is_off_keep_their_contrast_on_the_card_in_both_appearances() {
        for (name, palette) in [("light", Palette::light()), ("dark", Palette::dark())] {
            let off = colours(false, false, &palette);
            let on = colours(true, false, &palette);
            let off_ratio = crate::theme_contrast::contrast(off.ink, off.face);
            let on_ratio = crate::theme_contrast::contrast(on.ink, palette.fill);
            println!("{name}: off words {off_ratio:.2}, on words {on_ratio:.2}");
            assert!(
                off_ratio >= crate::theme_contrast::TEXT,
                "{name}: {off_ratio}"
            );
            assert!(
                on_ratio >= crate::theme_contrast::TEXT,
                "{name}: {on_ratio}"
            );
        }
    }

    fn rules_with(edit: impl FnOnce(&mut serde_json::Value)) -> Rules {
        let mut document: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sakura-core/tests/fixtures/engine-rules.json"
        ))
        .expect("the fixture");
        edit(&mut document);
        serde_json::from_value(document).expect("rules")
    }

    fn set(document: &mut serde_json::Value, id: &str, value: serde_json::Value) {
        for node in document["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == id {
                    rule["value"] = value.clone();
                }
            }
        }
    }

    /// Rules with agents: the build step runs on `account` (a Leeway name given for "a"), model `claude:sonnet`
    /// at `high`.
    fn rules_with_agents(account: &str) -> Rules {
        rules_with(|document| {
            document["agents"] = serde_json::json!([
                {"id": "a", "label": "Claude Work", "provider": "claude",
                 "models": [{"id": "sonnet", "label": "Sonnet 5.5", "efforts": ["high"]}]}
            ]);
            for node in document["nodes"].as_array_mut().expect("nodes") {
                for rule in node["rules"].as_array_mut().expect("rules") {
                    match rule["id"].as_str() {
                        Some("steps.build.model") => {
                            rule["value"] = "claude:sonnet".into();
                            rule["agent"] = serde_json::json!({"account": account, "provider": "claude", "model": "sonnet", "effort": "high"});
                        }
                        Some("steps.build.effort") => rule["value"] = "high".into(),
                        _ => {}
                    }
                }
            }
        })
    }

    #[test]
    fn a_step_box_says_its_account_and_model_in_words_with_the_effort() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let on_project = Pipeline::from_rules(Some(&rules_with_agents("project")));
        assert_eq!(
            on_project.agent_line(Node::Build).as_deref(),
            Some("Claude, Sonnet 5.5 high")
        );
        let pinned = Pipeline::from_rules(Some(&rules_with_agents("a")));
        assert_eq!(
            pinned.agent_line(Node::Build).as_deref(),
            Some("Claude Work, Sonnet 5.5 high")
        );
        assert_eq!(Pipeline::from_rules(None).agent_line(Node::Build), None);
    }

    #[test]
    fn a_plan_and_a_build_on_codex_read_as_codex_with_no_claude_word() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let rules = rules_with(|document| {
            document["agents"] = serde_json::json!([
                {"id": "b", "label": "Codex", "provider": "codex", "models": []}
            ]);
            for node in document["nodes"].as_array_mut().expect("nodes") {
                for rule in node["rules"].as_array_mut().expect("rules") {
                    if matches!(
                        rule["id"].as_str(),
                        Some("steps.plan.model" | "steps.build.model")
                    ) {
                        rule["value"] = "codex:gpt-5.5".into();
                        rule["agent"] = serde_json::json!({"account": "b", "provider": "codex",
                            "model": "gpt-5.5", "effort": "high"});
                    }
                }
            }
        });
        let pipeline = Pipeline::from_rules(Some(&rules));
        for node in [Node::Plan, Node::Build] {
            assert_eq!(
                pipeline.agent_line(node).as_deref(),
                Some("Codex, GPT-5.5 high"),
                "{node:?}"
            );
        }
    }

    #[test]
    fn the_final_review_box_says_the_code_reviewers_agent_though_the_engine_has_no_model_rule_for_it(
    ) {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let rules = rules_with(|document| {
            document["agents"] = serde_json::json!([
                {"id": "a", "label": "Claude Work", "provider": "claude",
                 "models": [{"id": "sonnet", "label": "Sonnet 5.5", "efforts": ["high"]}]}
            ]);
            for node in document["nodes"].as_array_mut().expect("nodes") {
                let rules = node["rules"].as_array_mut().expect("rules");
                rules.retain(|rule| rule["id"] != "reviews.final_review.model");
                for rule in rules {
                    if rule["id"] == "reviews.task_review.model" {
                        rule["value"] = "claude:sonnet".into();
                        rule["agent"] = serde_json::json!({"account": "project", "provider": "claude", "model": "sonnet", "effort": "high"});
                    }
                }
            }
            set(document, "reviews.final_review.on", true.into());
            set(document, "reviews.final_review.account", "project".into());
        });
        let pipeline = Pipeline::from_rules(Some(&rules));
        assert_eq!(
            pipeline.agent_line(Node::FinalReview).as_deref(),
            Some("Claude, Sonnet 5.5 high")
        );
    }

    #[test]
    fn the_standard_pipeline_runs_plan_both_reviews_and_checks_but_not_the_final_review() {
        let pipeline = Pipeline::from_rules(Some(&rules_with(|_| {})));
        let on: Vec<bool> = Node::ALL.iter().map(|node| pipeline.is_on(*node)).collect();
        assert_eq!(on, [true, true, true, true, true, false, true, true, true]);
    }

    #[test]
    fn with_no_rules_the_drawing_is_the_standard_pipeline_with_no_model() {
        let pipeline = Pipeline::from_rules(None);
        assert!(pipeline.is_on(Node::Plan) && pipeline.is_on(Node::TaskReview));
        assert!(!pipeline.is_on(Node::FinalReview));
        assert_eq!(pipeline.model(Node::Build), None);
    }

    #[test]
    fn turning_plan_off_bends_the_arrow_under_it_and_turns_the_plan_review_off_with_it() {
        let rules = rules_with(|document| set(document, "steps.plan.on", false.into()));
        let pipeline = Pipeline::from_rules(Some(&rules));
        assert!(!pipeline.is_on(Node::Plan) && !pipeline.is_on(Node::PlanReview));
        let lines = edges(&pipeline);
        assert!(lines
            .iter()
            .any(|edge| edge.points == sizes::START_BYPASS.to_vec()));
        assert!(!lines
            .iter()
            .any(|edge| edge.points == sizes::START_TO_PLAN.to_vec()));
    }

    #[test]
    fn the_final_review_sits_on_the_way_from_build_to_checks_when_it_is_on() {
        let rules = rules_with(|document| set(document, "reviews.final_review.on", true.into()));
        let lines = edges(&Pipeline::from_rules(Some(&rules)));
        assert!(lines
            .iter()
            .any(|edge| edge.points == sizes::BUILD_TO_FINAL.to_vec()));
        assert!(!lines
            .iter()
            .any(|edge| edge.points == sizes::BUILD_TO_CHECKS.to_vec()));
    }

    #[test]
    fn a_review_that_is_off_is_drawn_with_dashed_loops_and_the_guard_always_with_dashed_ties() {
        let rules = rules_with(|document| set(document, "reviews.plan_review.on", false.into()));
        let lines = edges(&Pipeline::from_rules(Some(&rules)));
        let loops = lines
            .iter()
            .filter(|edge| edge.has_arrow && edge.is_dashed)
            .count();
        assert_eq!(loops, 2, "the plan review's two arrows");
        assert_eq!(
            lines
                .iter()
                .filter(|edge| !edge.has_arrow && edge.is_dashed)
                .count(),
            4
        );
    }

    #[test]
    fn with_one_account_no_node_names_an_account() {
        let rules = rules_with(|document| document["accounts_count"] = 1.into());
        let pipeline = Pipeline::from_rules(Some(&rules));
        assert_eq!(pipeline.account(Node::Build), None);
        let many = Pipeline::from_rules(Some(&rules_with(|_| {})));
        assert_eq!(many.account(Node::Build).as_deref(), Some("Work"));
    }
}
