use super::controls;
use super::sheet::{Sheet, View};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use crate::ui::kit;
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Styled as _, Window,
};
use gpui_component::input::{InputState, Textarea, TextareaState};
use sakura_core::engine::accounts::Connector;
use sakura_core::engine::connectors::{
    ConnectorFields, ConnectorForm, ConnectorLabelEdit, ConnectorSelectors, GithubScope,
};

pub(super) struct Editor {
    pub(super) kind: &'static str,
    pub(super) original: Option<Connector>,
    label: Entity<InputState>,
    resource: Entity<InputState>,
    filter: Entity<InputState>,
    reads: Entity<InputState>,
    args: Entity<TextareaState>,
    is_reconnecting: bool,
    is_whole_repo: bool,
    _observers: Vec<gpui::Subscription>,
}

impl Editor {
    pub(super) fn new(
        kind: &'static str,
        original: Option<Connector>,
        is_reconnecting: bool,
        window: &mut Window,
        cx: &mut Context<Sheet>,
    ) -> Self {
        let saved = original.as_ref().and_then(selectors);
        let label = original
            .as_ref()
            .map(|row| row.label.clone())
            .unwrap_or_default();
        let reads = original
            .as_ref()
            .and_then(|row| row.reads.as_ref()?.text.clone())
            .unwrap_or_default();
        let whole_repo = saved.and_then(|scope| scope.whole_repo).unwrap_or(false);
        let resource_hint = original
            .as_ref()
            .and_then(|row| row.scope.as_ref())
            .and_then(|scope| {
                scope
                    .labels
                    .get("resource")
                    .or_else(|| scope.labels.get("host"))
            })
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| match kind {
                "feed" => "https://example.com/feed.xml".into(),
                "github" => "owner/repository".into(),
                _ => String::new(),
            });
        let filter_hint = saved
            .and_then(|scope| scope.branch.clone())
            .unwrap_or_default();
        let label =
            cx.new(|cx| controls::state_with(String::new(), window, cx).default_value(label));
        let resource = cx.new(|cx| controls::state_with(resource_hint, window, cx));
        let filter = cx.new(|cx| controls::state_with(filter_hint, window, cx));
        let reads = cx.new(|cx| {
            controls::state(Key("connectors.reads_label"), window, cx).default_value(reads)
        });
        let args = cx.new(|cx| TextareaState::new(window, cx).rows(3));
        let mut observers = [&label, &resource, &filter, &reads]
            .into_iter()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect::<Vec<_>>();
        observers.push(cx.observe(&args, |_, _, cx| cx.notify()));
        Self {
            kind,
            is_reconnecting,
            is_whole_repo: whole_repo,
            original,
            label,
            resource,
            filter,
            reads,
            args,
            _observers: observers,
        }
    }

    pub(super) fn title(&self) -> &'static str {
        if self.is_reconnecting {
            "connectors.reconnect"
        } else if self.original.is_some() {
            "connectors.edit"
        } else if self.kind == "feed" {
            "connectors.add_feed"
        } else {
            "connectors.add"
        }
    }

    pub(super) fn fields(&self, palette: &Palette, cx: &Context<Sheet>) -> Vec<AnyElement> {
        let mut fields = vec![field(
            "connector-name",
            "connectors.name",
            &self.label,
            palette,
        )];
        let resource_label = match self.kind {
            "github" => "connectors.repository",
            "linear" => "connectors.team",
            "notion" => "connectors.database",
            "feed" => "connectors.address",
            _ => "connectors.program",
        };
        if self.kind == "program" {
            fields.push(field(
                "connector-reads",
                "connectors.reads_label",
                &self.reads,
                palette,
            ));
            if self.original.is_some() {
                fields.push(controls::quiet(
                    tr(Key("connectors.saved_program")),
                    palette,
                ));
            }
        }
        fields.push(field(
            "connector-resource",
            resource_label,
            &self.resource,
            palette,
        ));
        if self.kind == "linear" {
            fields.push(field(
                "connector-filter",
                "connectors.recipient",
                &self.filter,
                palette,
            ));
        } else if self.kind == "github" {
            let view = cx.weak_entity();
            let toggle = kit::toggle(
                "connector-whole-repo",
                self.is_whole_repo,
                move |is_on, _, cx| {
                    drop(view.update(cx, |modal, cx| {
                        if let View::Edit(editor) = &mut modal.view {
                            editor.is_whole_repo = is_on;
                            cx.notify();
                        }
                    }));
                },
            );
            let toggle_row = crate::ui::settings_modal::group::row(
                "connector-whole-repo-row",
                tr(Key("connectors.whole_repo")),
                None,
                toggle,
                None,
                palette,
            );
            fields.push(crate::ui::settings_modal::group::group(
                vec![toggle_row],
                palette,
            ));
            fields.push(field(
                "connector-filter",
                "connectors.branch",
                &self.filter,
                palette,
            ));
        } else if self.kind == "program" {
            let args = Textarea::new(&self.args).appearance(false).bordered(false);
            fields.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::GAP_SMALL)
                    .child(controls::quiet(tr(Key("connectors.args")), palette))
                    .child(
                        div()
                            .w_full()
                            .bg(palette.fill)
                            .rounded(theme::RADIUS_CONTROL)
                            .child(args),
                    )
                    .into_any_element(),
            );
        }
        fields
    }

    pub(super) fn payload(&self, cx: &App) -> Option<String> {
        let label = typed(&self.label, cx);
        if label.is_empty() {
            return None;
        }
        let resource = typed(&self.resource, cx);
        let filter = typed(&self.filter, cx);
        let reads = typed(&self.reads, cx);
        let args = self.args.read(cx).value();
        let original_whole_repo = self
            .original
            .as_ref()
            .and_then(selectors)
            .and_then(|scope| scope.whole_repo)
            .unwrap_or(false);
        let is_label_edit = self.original.is_some()
            && !self.is_reconnecting
            && resource.is_empty()
            && filter.is_empty()
            && args.is_empty()
            && original_whole_repo == self.is_whole_repo;
        if is_label_edit {
            let reads = (self.kind == "program" && !reads.is_empty()).then_some(reads);
            return serde_json::to_string(&ConnectorLabelEdit { label, reads }).ok();
        }
        let fields = self.scope(resource, filter, reads, &args)?;
        serde_json::to_string(&ConnectorForm { label, fields }).ok()
    }

    fn scope(
        &self,
        resource: String,
        filter: String,
        reads: String,
        args: &str,
    ) -> Option<ConnectorFields> {
        let empty = ConnectorSelectors::default();
        let saved = self.original.as_ref().and_then(selectors).unwrap_or(&empty);
        let typed_or_saved =
            |saved: &Option<String>| nonempty(resource.clone()).or_else(|| saved.clone());
        Some(match self.kind {
            "github" => ConnectorFields::Github {
                repo: typed_or_saved(&saved.repo)?,
                scope: if self.is_whole_repo {
                    GithubScope::WholeRepo { whole_repo: true }
                } else {
                    GithubScope::Branch {
                        branch: nonempty(filter).or_else(|| saved.branch.clone())?,
                    }
                },
            },
            "linear" => ConnectorFields::Linear {
                team: typed_or_saved(&saved.team)?,
                recipient: nonempty(filter).or_else(|| saved.recipient.clone()),
            },
            "notion" => ConnectorFields::Notion {
                database: typed_or_saved(&saved.database)?,
            },
            "feed" => ConnectorFields::Feed {
                address: nonempty(resource)?,
            },
            "program" => ConnectorFields::Program {
                program: nonempty(resource)?,
                args: args.lines().map(str::to_string).collect(),
                reads: nonempty(reads)?,
            },
            _ => return None,
        })
    }
}

fn selectors(row: &Connector) -> Option<&ConnectorSelectors> {
    row.scope.as_ref().map(|scope| &scope.selectors)
}

fn typed(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().trim().to_string()
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn field(
    id: &'static str,
    label: &'static str,
    value: &Entity<InputState>,
    palette: &Palette,
) -> AnyElement {
    controls::labeled_field(id, tr(Key(label)), value, gpui::relative(1.), palette)
}
