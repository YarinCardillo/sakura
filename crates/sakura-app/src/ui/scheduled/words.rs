use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;
use sakura_core::engine::scheduled::{Run, ScheduledTask, Trigger};
use sakura_core::engine::snapshot::Snapshot;

pub(super) fn state(code: &str) -> &'static str {
    tr(Key(match code {
        "active" => "scheduled.active",
        "paused" => "scheduled.paused",
        "connector_unavailable" => "scheduled.blocked",
        "queued" => "scheduled.queued",
        "running" => "scheduled.running",
        "finished" => "scheduled.finished",
        "failed" => "scheduled.failed",
        "skipped" => "scheduled.skipped",
        _ => "scheduled.unknown",
    }))
}

pub(super) fn trigger(when: &Trigger, snapshot: Option<&Snapshot>) -> SharedString {
    match when {
        Trigger::Daily { time, timezone } => tr_with(
            Key("scheduled.daily_words"),
            &[("time", time), ("zone", timezone)],
        ),
        Trigger::Interval { minutes } => interval(*minutes),
        Trigger::ConnectorEvent {
            connector_id,
            event_kind,
        } => {
            let event = event(event_kind);
            let name = connector_name(snapshot, connector_id);
            tr_with(
                Key("scheduled.event_words"),
                &[("event", event), ("connector", name)],
            )
        }
    }
}

pub(super) fn interval(minutes: u16) -> SharedString {
    match minutes {
        60 => tr(Key("scheduled.hourly")).into(),
        1440 => tr(Key("scheduled.every_day")).into(),
        _ if minutes.is_multiple_of(60) => tr_with(
            Key("scheduled.hours_words"),
            &[("hours", &(minutes / 60).to_string())],
        ),
        _ => tr_with(
            Key("scheduled.interval_words"),
            &[("minutes", &minutes.to_string())],
        ),
    }
}

pub(in crate::ui) fn event(code: &str) -> &'static str {
    tr(Key(match code {
        "entry_added" | "new_item" => "scheduled.event.entry_added",
        "item_added" => "scheduled.event.item_added",
        "check_failed" => "scheduled.event.check_failed",
        "issue_assigned" => "scheduled.event.issue_assigned",
        "page_added" => "scheduled.event.page_added",
        _ => "scheduled.event.unknown",
    }))
}

fn connector_name<'a>(snapshot: Option<&'a Snapshot>, id: &str) -> &'a str {
    snapshot
        .and_then(|snapshot| snapshot.connectors.as_ref())
        .and_then(|connectors| connectors.iter().find(|connector| connector.id == id))
        .map(|connector| connector.label.as_str())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| tr(Key("scheduled.unknown_connector")))
}

pub(super) fn time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|time| {
            let local = time.with_timezone(&chrono::Local);
            tr_with(
                Key("scheduled.local_time"),
                &[
                    ("time", &local.format("%Y-%m-%d %H:%M").to_string()),
                    ("offset", &local.format("%:z").to_string()),
                ],
            )
            .to_string()
        })
        .unwrap_or_else(|_| tr(Key("scheduled.unknown")).into())
}

fn next_run(iso: &str, trigger: &Trigger) -> String {
    let Trigger::Daily { timezone, .. } = trigger else {
        return time(iso);
    };
    let instant = iso.parse::<jiff::Timestamp>();
    let zone = jiff::tz::TimeZone::get(timezone);
    match (instant, zone) {
        (Ok(instant), Ok(zone)) => tr_with(
            Key("scheduled.zoned_time"),
            &[
                (
                    "time",
                    &instant
                        .to_zoned(zone)
                        .strftime("%Y-%m-%d %H:%M")
                        .to_string(),
                ),
                ("zone", timezone),
            ],
        )
        .to_string(),
        _ => tr(Key("scheduled.unknown")).into(),
    }
}

pub(super) fn task_reason(code: &str, connector: Option<&str>) -> SharedString {
    let connector = connector.unwrap_or_else(|| tr(Key("scheduled.unknown_connector")));
    match code {
        "connector_disconnected" | "connector_unavailable" => tr_with(
            Key("scheduled.reason.connector_disconnected"),
            &[("connector", connector)],
        ),
        "connector_reconnected" => tr_with(
            Key("scheduled.reason.connector_reconnected"),
            &[("connector", connector)],
        ),
        _ => reason(code).into(),
    }
}

pub(in crate::ui) fn defaults(snapshot: &Snapshot) -> [Option<String>; 3] {
    let project = only(
        snapshot
            .projects
            .iter()
            .filter(|project| !project.folder_missing),
    )
    .map(|project| project.name.clone());
    let account = only(
        snapshot
            .accounts
            .iter()
            .filter(|account| !account.is_hidden()),
    )
    .map(|account| account.name().to_string());
    let preset = snapshot.engine.as_ref().and_then(|engine| {
        engine
            .preset_label
            .clone()
            .filter(|label| !label.is_empty())
            .or_else(|| {
                let id = engine.preset.as_deref()?;
                engine
                    .presets
                    .iter()
                    .find(|preset| preset.name == id)
                    .map(|preset| preset.words())
            })
    });
    [project, preset, account]
}

fn only<T>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

pub(super) fn task_title(task: &ScheduledTask) -> &str {
    if task.id == "morning-summary" && task.definition.title == "Morning summary" {
        tr(Key("engine_page.digest.title"))
    } else {
        &task.definition.title
    }
}

pub(in crate::ui) fn unresolved_default(field: &str) -> &'static str {
    tr(Key(match field {
        "scheduled.project" => "scheduled.no_project",
        "scheduled.account" => "scheduled.no_account",
        _ => "scheduled.automatic",
    }))
}

pub(super) fn properties(
    task: &ScheduledTask,
    snapshot: &Snapshot,
) -> Vec<(&'static str, SharedString)> {
    let definition = &task.definition;
    let project = definition
        .project
        .as_ref()
        .and_then(|id| snapshot.projects.iter().find(|p| &p.folder == id))
        .map(|p| p.name.as_str());
    let preset = definition
        .preset
        .as_ref()
        .and_then(|id| {
            snapshot
                .engine
                .as_ref()?
                .presets
                .iter()
                .find(|p| &p.name == id)
        })
        .map(|p| p.words());
    let account = definition
        .account
        .as_ref()
        .and_then(|id| snapshot.accounts.iter().find(|a| &a.id == id))
        .map(|a| a.name());
    let connectors = definition
        .connector_ids
        .iter()
        .map(|id| connector_name(Some(snapshot), id))
        .collect::<Vec<_>>()
        .join(", ");
    let [default_project, default_preset, default_account] = defaults(snapshot);
    let shown =
        |field, label: Option<&str>, selected: bool, default: Option<String>| -> SharedString {
            let value = if selected {
                label
                    .unwrap_or_else(|| tr(Key("scheduled.unknown")))
                    .to_string()
            } else {
                default.unwrap_or_else(|| unresolved_default(field).to_string())
            };
            value.into()
        };
    vec![
        (
            "scheduled.project",
            shown(
                "scheduled.project",
                project,
                definition.project.is_some(),
                default_project,
            ),
        ),
        (
            "scheduled.preset",
            shown(
                "scheduled.preset",
                preset.as_deref(),
                definition.preset.is_some(),
                default_preset,
            ),
        ),
        (
            "scheduled.account",
            shown(
                "scheduled.account",
                account,
                definition.account.is_some(),
                default_account,
            ),
        ),
        (
            "scheduled.connectors",
            if connectors.is_empty() {
                tr(Key("scheduled.none")).into()
            } else {
                connectors.into()
            },
        ),
        (
            "scheduled.next",
            task.next_at
                .as_deref()
                .map(|iso| next_run(iso, &definition.when))
                .map(Into::into)
                .or_else(|| {
                    matches!(definition.when, Trigger::ConnectorEvent { .. })
                        .then(|| trigger(&definition.when, Some(snapshot)))
                })
                .unwrap_or_else(|| tr(Key("scheduled.none")).into()),
        ),
        (
            "scheduled.may_start",
            tr(Key(if definition.may_start_lanes {
                "scheduled.yes"
            } else {
                "scheduled.no"
            }))
            .into(),
        ),
    ]
}

pub(super) fn run_note(run: &Run) -> SharedString {
    let trigger = run
        .trigger
        .as_ref()
        .map(|trigger| match trigger.kind.as_str() {
            "manual" => tr(Key("scheduled.manual")).into(),
            "connector_event" => trigger
                .source_label
                .clone()
                .unwrap_or_else(|| tr(Key("scheduled.trigger_event")).into()),
            _ => tr(Key("scheduled.trigger_clock")).into(),
        });
    let reason = run.reason.as_deref().map(reason);
    [trigger.as_deref(), reason]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

pub(in crate::ui) fn reason(code: &str) -> &'static str {
    tr(Key(match code {
        "run_unavailable" => "scheduled.reason.run_unavailable",
        "scheduled_store_unavailable" => "scheduled.reason.store",
        "launchd_reconciliation_failed" => "scheduled.reason.launchd",
        "legacy_digest_busy" => "scheduled.reason.migration_busy",
        "legacy_digest_config_unavailable" | "legacy_digest_state_unavailable" => {
            "scheduled.reason.migration_data"
        }
        "scheduled_provider_unavailable"
        | "scheduled_account_required"
        | "scheduled_adapter_unverified" => "scheduled.reason.migration_account",
        "legacy_agent_removal_pending" => "scheduled.reason.migration_pending",
        "stale_task" => "scheduled.reason.stale_task",
        "invalid_definition" => "scheduled.reason.invalid_definition",
        "invalid_when" => "scheduled.reason.invalid_when",
        "project_unavailable" | "lane_defaults_required" => "scheduled.reason.project",
        "preset_unavailable" => "scheduled.reason.preset",
        "account_unavailable" => "scheduled.reason.account",
        "connector_unavailable" | "unknown_connector" | "connector_registry_unavailable" => {
            "scheduled.reason.connector"
        }
        "unsupported_event" => "scheduled.reason.unsupported_event",
        "operation_conflict" | "invalid_operation" => "scheduled.reason.operation",
        "missed_occurrence" => "scheduled.reason.missed",
        "operator_paused" => "scheduled.reason.operator_paused",
        "task_changed" | "task_paused" | "task_deleted" => "scheduled.reason.changed",
        "adapter_unavailable" => "scheduled.reason.adapter",
        "all_sources_failed" => "scheduled.reason.sources",
        "interrupted" | "run_interrupted" => "scheduled.reason.interrupted",
        "acp_timeout" | "agent_timeout" | "runner_timeout" => "scheduled.reason.timeout",
        "action_unavailable" | "scheduled_action_unavailable" => "scheduled.reason.action",
        _ => "scheduled.reason.failed",
    }))
}
