//! "Connectors": the places the engine reads for the person (GitHub, Notion, Linear, feeds, their own
//! programs). Each signs in through its own tool or its own Keychain item: a token is never typed into
//! Sakura, the row says where to put it. A feed or a program is added by its address or path.

use super::controls;
use super::group::{button, group, row_with_mark, sentence};
use super::{AddForm, SettingsModal};
use crate::icons::ConnectorIcon;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Palette};
use crate::ui::chrome::{connector_mark, text};
use crate::ui::engine_acts::{self, CONNECT, DISCONNECT};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, Window,
};
use sakura_core::engine::accounts::Connector;
use std::rc::Rc;

/// The words of the form that adds a feed or a program: what each field is called and shows while it is
/// empty (an example of what to type), what the one button is named, and what is said when a field is empty.
struct FormWords {
    name_label: Key,
    name_hint: Key,
    address_label: Key,
    address_hint: Key,
    submit: Key,
    missing: Key,
}

/// One kind of connector: what names it for the engine, its words, and how it is signed in.
pub(in crate::ui) struct Kind {
    pub(in crate::ui) id: &'static str,
    name: Key,
    note: Key,
    /// Where the person puts the secret, when it is a Keychain item.
    keychain: Option<&'static str>,
    /// Whether the person adds as many as they like, each by an address or a path.
    pub(in crate::ui) is_many: bool,
    /// The form that adds one, for the kinds that are many.
    form: Option<FormWords>,
}

impl Kind {
    pub(super) fn title(&self) -> &'static str {
        tr(self.name)
    }
}

pub(in crate::ui) const KINDS: [Kind; 5] = [
    Kind {
        id: "github",
        name: Key("settings_modal.connectors.github"),
        note: Key("settings_modal.connectors.github_note"),
        keychain: None,
        is_many: false,
        form: None,
    },
    Kind {
        id: "notion",
        name: Key("settings_modal.connectors.notion"),
        note: Key("settings_modal.connectors.notion_note"),
        keychain: Some("sakura-connector-notion"),
        is_many: false,
        form: None,
    },
    Kind {
        id: "linear",
        name: Key("settings_modal.connectors.linear"),
        note: Key("settings_modal.connectors.linear_note"),
        keychain: Some("sakura-connector-linear"),
        is_many: false,
        form: None,
    },
    Kind {
        id: "feed",
        name: Key("settings_modal.connectors.feed"),
        note: Key("settings_modal.connectors.feed_note"),
        keychain: None,
        is_many: true,
        form: Some(FormWords {
            name_label: Key("settings_modal.connectors.feed_name_label"),
            name_hint: Key("settings_modal.connectors.feed_name_hint"),
            address_label: Key("settings_modal.connectors.feed_address_label"),
            address_hint: Key("settings_modal.connectors.feed_address_hint"),
            submit: Key("settings_modal.connectors.feed_submit"),
            missing: Key("settings_modal.connectors.feed_missing"),
        }),
    },
    Kind {
        id: "program",
        name: Key("settings_modal.connectors.program"),
        note: Key("settings_modal.connectors.program_note"),
        keychain: None,
        is_many: true,
        form: Some(FormWords {
            name_label: Key("settings_modal.connectors.program_name_label"),
            name_hint: Key("settings_modal.connectors.program_name_hint"),
            address_label: Key("settings_modal.connectors.program_address_label"),
            address_hint: Key("settings_modal.connectors.program_address_hint"),
            submit: Key("settings_modal.connectors.program_submit"),
            missing: Key("settings_modal.connectors.program_missing"),
        }),
    },
];

/// The fields of the add form of every kind that has one, each showing the example of its own kind.
pub(super) fn add_forms(
    window: &mut Window,
    cx: &mut Context<SettingsModal>,
) -> Vec<(&'static str, AddForm)> {
    KINDS
        .iter()
        .filter_map(|kind| kind.form.as_ref().map(|words| (kind.id, words)))
        .map(|(id, words)| {
            let name = cx.new(|cx| controls::state(words.name_hint, window, cx));
            let address = cx.new(|cx| controls::state(words.address_hint, window, cx));
            (id, AddForm { name, address })
        })
        .collect()
}

impl SettingsModal {
    /// The fields of the form that adds a connector of the kind `kind`.
    pub(super) fn add_form_of(&self, kind: &str) -> &AddForm {
        let (_, form) = self
            .add_forms
            .iter()
            .find(|(id, _)| *id == kind)
            .expect("every kind that is added has a form");
        form
    }

    /// The groups of "Connectors".
    pub(super) fn connectors(
        &mut self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.snapshot(cx).is_some_and(|snapshot| {
            snapshot.connector_controls.is_some() || snapshot.connectors_problem.is_some()
        }) {
            return self.versioned_connectors(palette, cx);
        }
        let listed: Option<Vec<Connector>> = self
            .snapshot(cx)
            .and_then(|snapshot| snapshot.connectors.clone());
        let intro = div()
            .debug_selector(|| "settings-connectors-intro".into())
            .child(sentence(
                tr(Key("settings_modal.connectors.intro")),
                palette,
            ))
            .into_any_element();
        let Some(listed) = listed else {
            return vec![
                intro,
                sentence(tr(Key("settings_modal.connectors.unreported")), palette),
            ];
        };
        let rows = KINDS
            .iter()
            .map(|kind| self.connector_row(kind, &listed, palette, cx));
        vec![intro, group(rows.collect(), palette)]
    }

    /// Shows the connector group reached from another page.
    pub(in crate::ui) fn show_connector(&mut self, kind: &str, cx: &mut Context<Self>) {
        self.open_connector = Some(kind.to_string());
        if self
            .snapshot(cx)
            .is_some_and(|snapshot| snapshot.connector_controls.is_some())
        {
            if let Some(position) = super::versioned_connectors::group_position(kind) {
                self.scroll.scroll_to_item(position);
            }
        }
        cx.notify();
    }

    fn toggle_connector(&mut self, kind: &str, cx: &mut Context<Self>) {
        let is_open = self.open_connector.as_deref() == Some(kind);
        self.open_connector = (!is_open).then(|| kind.to_string());
        self.form_problem = None;
        self.reveal_from = (!is_open).then(|| self.scroll.max_offset().y);
        cx.notify();
    }

    fn connector_row(
        &self,
        kind: &Kind,
        all: &[Connector],
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mine: Vec<&Connector> = all
            .iter()
            .filter(|connector| connector.kind == kind.id)
            .collect();
        let connected = mine.iter().filter(|connector| connector.connected).count();
        let id = format!("settings-connector-{}", kind.id);
        let state = text(theme::TEXT_LABEL, palette.text_tertiary)
            .w(sizes::CONNECTOR_STATUS_WIDTH)
            .text_right()
            .child(state_words(kind, connected));
        let is_open = self.open_connector.as_deref() == Some(kind.id);
        let action =
            (!(is_open && kind.is_many)).then(|| self.connector_button(kind, connected, &id, cx));
        let controls = div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(state)
            .child(div().w(sizes::CONNECTOR_ACTION_WIDTH).children(action));
        let mut below = vec![engine_acts::line(&self.acts, &id, palette, cx)];
        if is_open {
            below.push(Some(self.connector_panel(kind, &mine, &id, palette, cx)));
        } else if let (Some(item), 0) = (kind.keychain, connected) {
            let literal = format!("`{item}`");
            let words = tr_with(
                Key("settings_modal.connectors.keychain"),
                &[("item", &literal)],
            );
            below.push(Some(
                text(theme::TEXT_LABEL, palette.text_tertiary)
                    .pl(theme::kit::scale::ICON_REGULAR + theme::kit::scale::SPACE_M)
                    .child(crate::ui::path_text::plain_with_paths(
                        &words,
                        palette.text_tertiary,
                        palette.text_secondary,
                    ))
                    .into_any_element(),
            ));
        }
        let below: Vec<AnyElement> = below.into_iter().flatten().collect();
        let below = (!below.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(theme::GAP)
                .children(below)
                .into_any_element()
        });
        let mark = ConnectorIcon::of_kind(kind.id)
            .map(|icon| connector_mark(icon, format!("{id}-mark"), palette));
        row_with_mark(
            id,
            mark,
            tr(kind.name),
            Some(tr(kind.note).into()),
            controls,
            below,
            palette,
        )
    }

    /// Connect, Manage or Add: the one button a kind's row has.
    fn connector_button(
        &self,
        kind: &Kind,
        connected: usize,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (word, opens) = match (kind.is_many, connected) {
            (true, _) => (Key("settings_modal.connectors.add"), true),
            (false, 0) => (Key("settings_modal.connectors.connect"), false),
            (false, _) => (Key("settings_modal.connectors.manage"), true),
        };
        let (this, acts) = (cx.entity().downgrade(), self.acts.clone());
        let (kind_id, press_key) = (kind.id, id.to_string());
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            if opens {
                drop(this.update(cx, |modal, cx| modal.toggle_connector(kind_id, cx)));
            } else {
                let act = CONNECT.act(&[("kind", kind_id)]);
                acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
            }
        });
        button(format!("{id}-button"), tr(word), press, cx)
            .w_full()
            .into_any_element()
    }

    /// What an open row holds: each connector of the kind with its Disconnect, and for a feed or a program
    /// the field to add another in.
    fn connector_panel(
        &self,
        kind: &Kind,
        mine: &[&Connector],
        id: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines: Vec<AnyElement> = mine
            .iter()
            .map(|connector| self.connector_line(connector, kind, id, palette, cx))
            .collect();
        let adding = kind.is_many.then(|| self.add_form(kind, id, palette, cx));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .children(lines)
            .children(adding)
            .into_any_element()
    }

    fn connector_line(
        &self,
        connector: &Connector,
        kind: &Kind,
        id: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = if connector.label.trim().is_empty() || connector.label == connector.id {
            tr(kind.name)
        } else {
            &connector.label
        };
        let state = if connector.connected {
            Key("settings_modal.connectors.connected")
        } else {
            Key("settings_modal.connectors.not_connected")
        };
        let words = format!("{name}, {}", tr(state));
        let reads = self.reads_of(connector, cx);
        let (acts, press_key, name_input) =
            (self.acts.clone(), id.to_string(), connector.id.clone());
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            let act = DISCONNECT.act(&[("name", &name_input)]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
        });
        let disconnect = button(
            format!("{id}-disconnect-{}", connector.id),
            tr(Key("settings_modal.connectors.disconnect")),
            press,
            cx,
        );
        div()
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        text(theme::TEXT_SMALL, palette.text_secondary)
                            .child(SharedString::from(words)),
                    )
                    .children(reads.map(|reads| {
                        text(theme::TEXT_LABEL, palette.text_tertiary)
                            .debug_selector(|| format!("{id}-reads-{}", connector.id))
                            .child(reads)
                    })),
            )
            .child(disconnect)
            .into_any_element()
    }

    /// What the digest reads through `connector`, in the engine's phrase for its source; none when the digest
    /// has no source on it or the engine gave no phrase.
    fn reads_of(&self, connector: &Connector, cx: &Context<Self>) -> Option<SharedString> {
        if let Some(reads) = connector
            .reads
            .as_ref()
            .and_then(|reads| reads.text.as_deref())
        {
            return Some(reads.to_string().into());
        }
        let snapshot = self.snapshot(cx)?;
        let sources = &snapshot.digest.as_ref()?.sources;
        let source = sources.iter().find(|source| {
            source.id == connector.id || source.connector.as_deref() == Some(&connector.id)
        })?;
        (!source.reads.is_empty()).then(|| SharedString::from(source.reads.clone()))
    }

    /// The form that adds a feed or a program: each field named and showing an example, a refusal in a
    /// sentence, and the one button named for its action beside Cancel.
    fn add_form(
        &self,
        kind: &Kind,
        id: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let words = kind.form.as_ref().expect("a kind that is added has a form");
        let form = self.add_form_of(kind.id);
        let fields = [
            ("name", words.name_label, &form.name),
            ("address", words.address_label, &form.address),
        ]
        .map(|(part, label, input)| {
            controls::labeled_field(
                format!("{id}-{part}"),
                tr(label),
                input,
                sizes::FORM_FIELD_WIDTH,
                palette,
            )
        });
        let problem = self.form_problem.clone().map(|problem| {
            text(theme::TEXT_LABEL, palette.danger)
                .debug_selector({
                    let selector = format!("{id}-problem");
                    move || selector.clone()
                })
                .child(problem)
                .into_any_element()
        });
        let buttons = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(self.submit_button(kind, words, id, cx))
            .child(self.cancel_button(kind, id, cx));
        div()
            .flex()
            .flex_col()
            .gap(theme::GAP_WIDE)
            .children(fields)
            .children(problem)
            .child(buttons)
            .into_any_element()
    }

    fn submit_button(
        &self,
        kind: &Kind,
        words: &FormWords,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let form = self.add_form_of(kind.id);
        let (name_state, address_state) = (form.name.clone(), form.address.clone());
        let (this, acts, press_key, kind_id) = (
            cx.entity().downgrade(),
            self.acts.clone(),
            id.to_string(),
            kind.id,
        );
        let missing: SharedString = tr(words.missing).into();
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            let name = name_state.read(cx).value().trim().to_string();
            let address = address_state.read(cx).value().trim().to_string();
            if name.is_empty() || address.is_empty() {
                let problem = missing.clone();
                return drop(this.update(cx, |modal, cx| {
                    modal.form_problem = Some(problem);
                    cx.notify();
                }));
            }
            drop(this.update(cx, |modal, _| modal.form_problem = None));
            let act = CONNECT.act(&[("kind", kind_id), ("label", &name), ("address", &address)]);
            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
        });
        controls::primary_button(format!("{id}-add"), tr(words.submit), press, cx)
    }

    fn cancel_button(&self, kind: &Kind, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let (this, kind_id) = (cx.entity().downgrade(), kind.id);
        let press = Rc::new(move |_: &mut Window, cx: &mut App| {
            drop(this.update(cx, |modal, cx| modal.toggle_connector(kind_id, cx)));
        });
        button(format!("{id}-cancel"), tr(Key("common.cancel")), press, cx).into_any_element()
    }
}

/// The examples an empty add form of the kind `kind` shows in its two fields.
#[cfg(test)]
pub(super) fn examples_of(kind: &str) -> Option<(SharedString, SharedString)> {
    let words = KINDS
        .iter()
        .find(|candidate| candidate.id == kind)?
        .form
        .as_ref()?;
    Some((tr(words.name_hint).into(), tr(words.address_hint).into()))
}

/// What a row says of its state: connected or not for a sign-in, how many were added for the others.
fn state_words(kind: &Kind, connected: usize) -> SharedString {
    match (kind.is_many, connected) {
        (_, 0) => tr(Key("settings_modal.connectors.not_connected")).into(),
        (true, count) => tr_with(
            Key("settings_modal.connectors.added"),
            &[("count", &count.to_string())],
        ),
        (false, _) => tr(Key("settings_modal.connectors.connected")).into(),
    }
}
