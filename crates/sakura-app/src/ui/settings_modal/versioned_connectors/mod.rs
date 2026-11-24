mod editor;
mod sheet;
pub(in crate::ui) mod words;

use self::sheet::{open, Selection};
use super::{connectors::KINDS, controls, group, SettingsModal};
use crate::l10n::{tr, Key};
use crate::theme::{self, settings_modal as sizes, Palette};
use gpui::{div, AnyElement, Context, ParentElement as _, Styled as _};
use sakura_core::engine::accounts::Connector;
use sakura_core::engine::connectors::{ConnectorControls, CONTROLS_VERSION};
use std::rc::Rc;

const GROUPS: [&str; 5] = ["github", "linear", "notion", "feed", "program"];

pub(super) fn group_position(kind: &str) -> Option<usize> {
    GROUPS
        .iter()
        .position(|id| *id == kind)
        .map(|index| index + 1)
}

impl SettingsModal {
    pub(super) fn versioned_connectors(
        &self,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut content = vec![group::sentence(tr(Key("connectors.intro")), palette)];
        let Some(snapshot) = self.snapshot(cx) else {
            return content;
        };
        if snapshot.connectors_problem.is_some() {
            content.push(group::sentence(
                tr(Key("connectors.registry_unavailable")),
                palette,
            ));
            return content;
        }
        let Some(rows) = snapshot.connectors.as_deref() else {
            return Vec::new();
        };
        let controls = snapshot.connector_controls.as_ref().filter(|controls| {
            controls.version == CONTROLS_VERSION
                && controls.target == crate::ui::engine_acts::CONNECTORS_TARGET
        });
        for kind in GROUPS {
            content.push(self.connection_group(kind, rows, controls, palette, cx));
        }
        content
    }

    fn connection_group(
        &self,
        kind: &'static str,
        rows: &[Connector],
        advertised: Option<&ConnectorControls>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open_kind = self.open_connector.as_deref() == Some(kind);
        let mut group_rows =
            vec![self.connection_group_heading(kind, open_kind, advertised, palette, cx)];
        group_rows.extend(
            rows.iter()
                .filter(|row| row.kind == kind)
                .map(|row| self.connection_row(row, palette, cx)),
        );
        group::group(group_rows, palette)
    }

    fn connection_group_heading(
        &self,
        kind: &'static str,
        open_kind: bool,
        advertised: Option<&ConnectorControls>,
        palette: &Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = format!("settings-connector-{kind}");
        let title = kind_title(kind);
        let add = if allows(advertised, "connectoradd") {
            let (source, facts) = (self.source.clone(), self.facts.clone());
            let key = if kind == "program" {
                "connectors.add_custom"
            } else {
                "connectors.add"
            };
            Some(group::button(
                format!("connector-add-{kind}"),
                tr(Key(key)),
                Rc::new(move |window, cx| {
                    open(
                        source.clone(),
                        facts.clone(),
                        Selection::Add(kind),
                        window,
                        cx,
                    )
                }),
                cx,
            ))
        } else {
            None
        };
        group::row(
            id,
            title,
            open_kind.then(|| guidance(kind).into()),
            div().children(add),
            None,
            palette,
        )
    }

    fn connection_row(&self, row: &Connector, palette: &Palette, cx: &Context<Self>) -> AnyElement {
        let id = row.id.clone();
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let manage = group::button(
            format!("connector-manage-{id}"),
            tr(Key("settings_modal.connectors.manage")),
            Rc::new(move |window, cx| {
                open(
                    source.clone(),
                    facts.clone(),
                    Selection::Manage(id.clone()),
                    window,
                    cx,
                )
            }),
            cx,
        );
        let trailing = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(
                div()
                    .w(sizes::CONNECTOR_STATUS_WIDTH)
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_secondary)
                    .child(words::status(row)),
            )
            .child(manage);
        group::row(
            format!("connector-instance-{}", row.id),
            words::name(row),
            Some(words::reads(row)),
            trailing,
            None,
            palette,
        )
    }
}

fn allows(controls: Option<&ConnectorControls>, key: &str) -> bool {
    controls.is_some_and(|controls| {
        crate::ui::engine_acts::connector_request(controls, key, Default::default()).is_some()
    })
}

fn kind_title(kind: &str) -> &'static str {
    if kind == "program" {
        return tr(Key("connectors.custom"));
    }
    KINDS
        .iter()
        .find(|choice| choice.id == kind)
        .map_or_else(|| tr(Key("connectors.unnamed")), |choice| choice.title())
}

fn guidance(kind: &str) -> &'static str {
    tr(Key(match kind {
        "github" => "connectors.guidance.github",
        "linear" => "connectors.guidance.linear",
        "notion" => "connectors.guidance.notion",
        "feed" => "connectors.guidance.feed",
        _ => "connectors.guidance.program",
    }))
}
