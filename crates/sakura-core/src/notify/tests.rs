use super::*;

fn lavoro(name: &str, stage: Stage, fact: &str) -> Lavoro {
    Lavoro {
        name: name.into(),
        stage,
        fact: fact.into(),
    }
}

fn note(kind: NoteKind, task: &str) -> Note {
    Note {
        kind,
        task: task.into(),
    }
}

/// The notifier after it saw `first` once: what is there at launch is never news.
fn seen(first: &[Lavoro]) -> Notifier {
    let mut notifier = Notifier::default();
    let at_launch = notifier.observe(first, false);
    assert!(at_launch.notes.is_empty(), "nothing is news at launch");
    notifier
}

#[test]
fn a_task_that_starts_to_need_the_person_is_told_once_while_sakura_is_behind() {
    let mut notifier = seen(&[lavoro("a", Stage::Working, "")]);
    let needs = [lavoro("a", Stage::NeedsPerson, "permission-1")];
    let decision = notifier.observe(&needs, false);
    assert_eq!(decision.notes, [note(NoteKind::NeedsPerson, "a")]);
    assert_eq!(decision.badge, 1);
    // The same state seen again says nothing more; the badge stays.
    let again = notifier.observe(&needs, false);
    assert_eq!((again.notes.len(), again.badge), (0, 1));
}

#[test]
fn an_agent_that_finishes_is_told_once_and_needs_nobody() {
    let mut notifier = seen(&[lavoro("a", Stage::Working, "")]);
    let done = [lavoro("a", Stage::Done, "head-1")];
    let decision = notifier.observe(&done, false);
    assert_eq!(decision.notes, [note(NoteKind::Finished, "a")]);
    assert_eq!(decision.badge, 0);
    assert!(notifier.observe(&done, false).notes.is_empty());
    // A task that was not working did not finish: nothing ended.
    let mut quiet = seen(&[lavoro("b", Stage::Quiet, "")]);
    assert!(quiet
        .observe(&[lavoro("b", Stage::Done, "")], false)
        .notes
        .is_empty());
}

#[test]
fn nothing_is_told_while_sakura_is_in_front_and_not_afterwards_either() {
    let mut notifier = seen(&[lavoro("a", Stage::Working, "")]);
    let needs = [lavoro("a", Stage::NeedsPerson, "permission-1")];
    let in_front = notifier.observe(&needs, true);
    assert_eq!(
        (in_front.notes.len(), in_front.badge),
        (0, 1),
        "the badge still counts it"
    );
    assert!(
        notifier.observe(&needs, false).notes.is_empty(),
        "the person saw it"
    );
}

#[test]
fn the_same_fact_is_never_told_twice_and_a_new_one_is() {
    let mut notifier = seen(&[lavoro("a", Stage::Working, "")]);
    let needs = |fact: &str| [lavoro("a", Stage::NeedsPerson, fact)];
    let working = [lavoro("a", Stage::Working, "")];
    assert_eq!(
        notifier.observe(&needs("permission-1"), false).notes.len(),
        1
    );
    // A read in between shows it working for a moment, then the same request again.
    notifier.observe(&working, false);
    assert!(notifier
        .observe(&needs("permission-1"), false)
        .notes
        .is_empty());
    notifier.observe(&working, false);
    assert_eq!(
        notifier.observe(&needs("permission-2"), false).notes.len(),
        1
    );
}

#[test]
fn a_task_made_while_sakura_runs_is_news_and_one_that_went_is_forgotten() {
    let mut notifier = seen(&[lavoro("a", Stage::Working, "")]);
    let both = [
        lavoro("a", Stage::Working, ""),
        lavoro("b", Stage::NeedsPerson, "question-1"),
    ];
    assert_eq!(
        notifier.observe(&both, false).notes,
        [note(NoteKind::NeedsPerson, "b")]
    );
    notifier.observe(&[lavoro("a", Stage::Working, "")], false);
    assert_eq!(
        notifier.told_count(),
        0,
        "nothing is kept of a task that is gone"
    );
    // The same name made again with the same fact is a new task: told again.
    assert_eq!(notifier.observe(&both, false).notes.len(), 1);
}

#[test]
fn the_badge_counts_the_tasks_that_need_the_person() {
    let mut notifier = Notifier::default();
    let lavori = [
        lavoro("a", Stage::NeedsPerson, "1"),
        lavoro("b", Stage::NeedsPerson, "2"),
        lavoro("c", Stage::Done, "3"),
        lavoro("d", Stage::Working, ""),
        lavoro("e", Stage::Quiet, ""),
    ];
    assert_eq!(notifier.observe(&lavori, true).badge, 2);
    assert_eq!(notifier.observe(&[], true).badge, 0);
}

#[test]
fn many_facts_at_once_are_a_few_notes_and_the_rest_is_never_a_later_burst() {
    let names: Vec<String> = (0..NOTES_MAX + 5).map(|n| format!("t{n}")).collect();
    let all = |stage: Stage| -> Vec<Lavoro> {
        names.iter().map(|name| lavoro(name, stage, "f")).collect()
    };
    let mut notifier = seen(&all(Stage::Working));
    let decision = notifier.observe(&all(Stage::NeedsPerson), false);
    assert_eq!(decision.notes.len(), NOTES_MAX);
    assert_eq!(decision.badge as usize, NOTES_MAX + 5);
    assert!(notifier
        .observe(&all(Stage::NeedsPerson), false)
        .notes
        .is_empty());
}

#[test]
fn a_list_no_window_holds_is_bounded_and_a_name_twice_counts_once() {
    let twice = [
        lavoro("a", Stage::NeedsPerson, "1"),
        lavoro("a", Stage::NeedsPerson, "1"),
    ];
    let mut notifier = seen(&[]);
    let decision = notifier.observe(&twice, false);
    assert_eq!((decision.notes.len(), decision.badge), (1, 1));
    let many: Vec<Lavoro> = (0..LAVORI_MAX + 10)
        .map(|n| lavoro(&format!("t{n}"), Stage::NeedsPerson, "f"))
        .collect();
    let mut notifier = Notifier::default();
    assert_eq!(notifier.observe(&many, true).badge as usize, LAVORI_MAX);
}

#[test]
fn the_badge_is_written_short() {
    assert_eq!(badge_label(0), None);
    assert_eq!(badge_label(3).as_deref(), Some("3"));
    assert_eq!(badge_label(99).as_deref(), Some("99"));
    assert_eq!(badge_label(100).as_deref(), Some("99+"));
    assert_eq!(badge_label(u32::MAX).as_deref(), Some("99+"));
}
