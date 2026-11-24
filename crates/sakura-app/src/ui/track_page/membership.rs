use super::{form, name, request};
use crate::data::{engine_facts::EngineFacts, source::Source};
use crate::l10n::{tr, Key};
use crate::theme;
use crate::ui::settings_modal::controls;
use gpui::{
    div, AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::engine::tracks::Track;
use std::collections::BTreeMap;
use std::rc::Rc;

type OpenTrack = Rc<dyn Fn(String, &mut App)>;

pub(in crate::ui) struct Membership<'a> {
    pub snapshot: &'a Snapshot,
    pub current: Option<&'a Track>,
    pub inputs: BTreeMap<String, String>,
    pub title: Option<String>,
    pub can_link: bool,
    pub can_unlink: bool,
    pub open_track: OpenTrack,
}

pub(in crate::ui) fn membership_control(
    id: &'static str,
    membership: Membership<'_>,
    facts: &gpui::Entity<EngineFacts>,
    source: &Source,
    cx: &App,
) -> Option<AnyElement> {
    let mut choices = Vec::new();
    let pick = |label: SharedString, spec: form::FormSpec| {
        let (facts, source) = (facts.clone(), source.clone());
        controls::Choice {
            label,
            is_on: false,
            on_pick: Rc::new(move |window, cx| {
                form::open(spec.clone(), facts.clone(), source.clone(), window, cx)
            }),
        }
    };
    if let Some(current) = membership.current {
        let (open, track) = (membership.open_track.clone(), current.id.clone());
        choices.push(controls::Choice {
            label: tr(Key("track.open_link")).into(),
            is_on: false,
            on_pick: Rc::new(move |_: &mut Window, cx| open(track.clone(), cx)),
        });
        if let Some(action) = current
            .action("trackunlink")
            .filter(|_| membership.can_unlink)
        {
            let spec = form::FormSpec::action(
                "track.unlink",
                request(action, current.target(), membership.inputs.clone()),
            );
            choices.push(pick(tr(Key("track.unlink")).into(), spec));
        }
    }
    if membership.can_link {
        choices.extend(
            membership
                .snapshot
                .tracks
                .rows()
                .iter()
                .filter_map(|track| {
                    if membership
                        .current
                        .is_some_and(|current| track.id == current.id)
                    {
                        return None;
                    }
                    let action = track.action("tracklink")?;
                    let mut inputs = membership.inputs.clone();
                    inputs.extend(
                        membership
                            .title
                            .as_ref()
                            .map(|title| ("title".into(), title.clone())),
                    );
                    let label = tr(Key("track.link"));
                    let spec = form::FormSpec::action(
                        "track.link",
                        request(action, track.target(), inputs),
                    );
                    Some(pick(format!("{label}: {}", name(track)).into(), spec))
                }),
        );
    }
    if choices.is_empty() {
        return None;
    }
    let label = membership
        .current
        .map_or_else(|| tr(Key("track.link")).into(), |track| name(track).into());
    Some(
        div()
            .max_w(theme::SIDEBAR_WIDTH)
            .min_w_0()
            .overflow_hidden()
            .child(controls::menu(id, label, choices, cx))
            .into_any_element(),
    )
}
