use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Board, ChecklistItem, Task};

pub type TaskHistory = HashMap<u64, Vec<TaskHistoryEvent>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovementTracking {
    All,
    Only(u64),
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskHistoryEvent {
    pub at: DateTime<Utc>,
    pub kind: TaskHistoryKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "details", rename_all = "snake_case")]
pub enum TaskHistoryKind {
    Created,
    Changed {
        field: String,
        from: String,
        to: String,
    },
    Added {
        field: String,
        value: String,
    },
    Removed {
        field: String,
        value: String,
    },
    TagAdded(String),
    TagRemoved(String),
    Moved {
        from_column: String,
        from_position: usize,
        to_column: String,
        to_position: usize,
    },
}

pub fn checklist_status_item(field: &str) -> Option<String> {
    let encoded = field.strip_prefix("checklist status for ")?;
    Some(serde_json::from_str(encoded).unwrap_or_else(|_| encoded.trim_matches('"').to_owned()))
}

pub fn describe_checklist_status_change(field: &str, to: &str) -> Option<String> {
    let item = checklist_status_item(field)?;
    match to {
        "complete" => Some(format!("Checked {item}")),
        "incomplete" => Some(format!("Unchecked {item}")),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct TaskLocation<'a> {
    column_id: u64,
    column_title: &'a str,
    position: usize,
    task: &'a Task,
}

/// Derives semantic task events from board snapshots. This remains available for
/// the one-time v1 migration; v2 persists the resulting events in its Git-backed
/// append-only ledger instead of diffing snapshots during normal operation.
#[cfg(test)]
pub fn derive_task_history(snapshots: &[(DateTime<Utc>, Board)]) -> TaskHistory {
    let mut histories = TaskHistory::new();
    let mut previous: Option<&Board> = None;

    for (committed_at, board) in snapshots {
        merge_history(
            &mut histories,
            derive_board_changes(previous, board, *committed_at),
        );
        previous = Some(board);
    }

    histories
}

#[cfg(test)]
pub fn derive_board_changes(
    before: Option<&Board>,
    after: &Board,
    committed_at: DateTime<Utc>,
) -> TaskHistory {
    derive_board_changes_with_movement(before, after, committed_at, MovementTracking::All)
}

pub fn derive_board_changes_with_movement(
    before: Option<&Board>,
    after: &Board,
    committed_at: DateTime<Utc>,
    movement_tracking: MovementTracking,
) -> TaskHistory {
    let mut histories = TaskHistory::new();
    let previous_locations = before.map(|board| {
        task_locations(board)
            .map(|location| (location.task.id, location))
            .collect::<HashMap<_, _>>()
    });
    for location in task_locations(after) {
        let events = histories.entry(location.task.id).or_default();
        let previous_location = previous_locations
            .as_ref()
            .and_then(|locations| locations.get(&location.task.id))
            .copied();
        match previous_location {
            Some(before) => {
                derive_changes(before, location, committed_at, events, movement_tracking)
            }
            None => events.push(TaskHistoryEvent {
                // The task creation time is more precise than its commit time and
                // also preserves the correct date for boards imported into Git.
                at: location.task.created_at,
                kind: TaskHistoryKind::Created,
            }),
        }
    }
    histories.retain(|_, events| !events.is_empty());
    histories
}

#[cfg(test)]
pub fn merge_history(target: &mut TaskHistory, source: TaskHistory) {
    for (task_id, events) in source {
        target.entry(task_id).or_default().extend(events);
    }
}

fn derive_changes(
    before: TaskLocation<'_>,
    after: TaskLocation<'_>,
    at: DateTime<Utc>,
    events: &mut Vec<TaskHistoryEvent>,
    movement_tracking: MovementTracking,
) {
    if before.task.title != after.task.title {
        changed(events, at, "title", &before.task.title, &after.task.title);
    }
    derive_optional_text_change(
        events,
        at,
        "description",
        &before.task.description,
        &after.task.description,
    );
    derive_due_date_change(events, at, before.task, after.task);
    derive_tag_changes(events, at, &before.task.tags, &after.task.tags);
    derive_checklist_changes(events, at, &before.task.checklist, &after.task.checklist);

    let track_movement = match movement_tracking {
        MovementTracking::All => true,
        MovementTracking::Only(task_id) => task_id == after.task.id,
        MovementTracking::None => false,
    };
    if track_movement && (before.column_id != after.column_id || before.position != after.position)
    {
        events.push(TaskHistoryEvent {
            at,
            kind: TaskHistoryKind::Moved {
                from_column: before.column_title.to_owned(),
                from_position: before.position,
                to_column: after.column_title.to_owned(),
                to_position: after.position,
            },
        });
    }
}

fn derive_optional_text_change(
    events: &mut Vec<TaskHistoryEvent>,
    at: DateTime<Utc>,
    field: &str,
    before: &str,
    after: &str,
) {
    if before == after {
        return;
    }
    match (before.is_empty(), after.is_empty()) {
        (true, false) => added(events, at, field, after),
        (false, true) => removed(events, at, field, before),
        (false, false) => changed(events, at, field, before, after),
        (true, true) => {}
    }
}

fn derive_due_date_change(
    events: &mut Vec<TaskHistoryEvent>,
    at: DateTime<Utc>,
    before: &Task,
    after: &Task,
) {
    if before.due_date == after.due_date {
        return;
    }
    match (before.due_date, after.due_date) {
        (None, Some(date)) => added(events, at, "due date", &date.to_string()),
        (Some(date), None) => removed(events, at, "due date", &date.to_string()),
        (Some(from), Some(to)) => {
            changed(events, at, "due date", &from.to_string(), &to.to_string())
        }
        (None, None) => {}
    }
}

fn derive_tag_changes(
    events: &mut Vec<TaskHistoryEvent>,
    at: DateTime<Utc>,
    before: &[String],
    after: &[String],
) {
    let before_names = before
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let after_names = after
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<HashSet<_>>();

    for tag in before {
        if !after_names.contains(&tag.to_ascii_lowercase()) {
            events.push(TaskHistoryEvent {
                at,
                kind: TaskHistoryKind::TagRemoved(tag.clone()),
            });
        }
    }
    for tag in after {
        if !before_names.contains(&tag.to_ascii_lowercase()) {
            events.push(TaskHistoryEvent {
                at,
                kind: TaskHistoryKind::TagAdded(tag.clone()),
            });
        }
    }
}

fn derive_checklist_changes(
    events: &mut Vec<TaskHistoryEvent>,
    at: DateTime<Utc>,
    before: &[ChecklistItem],
    after: &[ChecklistItem],
) {
    if before == after {
        return;
    }

    if let Some((index, item)) = single_insertion(before, after) {
        added(
            events,
            at,
            &format!("checklist item {}", index + 1),
            &item.text,
        );
        return;
    }
    if let Some((index, item)) = single_removal(before, after) {
        removed(
            events,
            at,
            &format!("checklist item {}", index + 1),
            &item.text,
        );
        return;
    }

    for (index, (old, new)) in before.iter().zip(after).enumerate() {
        if old.text != new.text {
            changed(
                events,
                at,
                &format!("checklist item {}", index + 1),
                &old.text,
                &new.text,
            );
        }
        if old.completed != new.completed {
            changed(
                events,
                at,
                &format!("checklist status for {:?}", new.text),
                if old.completed {
                    "complete"
                } else {
                    "incomplete"
                },
                if new.completed {
                    "complete"
                } else {
                    "incomplete"
                },
            );
        }
    }
    for (index, item) in after.iter().enumerate().skip(before.len()) {
        added(
            events,
            at,
            &format!("checklist item {}", index + 1),
            &item.text,
        );
    }
    for (index, item) in before.iter().enumerate().skip(after.len()) {
        removed(
            events,
            at,
            &format!("checklist item {}", index + 1),
            &item.text,
        );
    }
}

fn single_insertion<'a>(
    before: &[ChecklistItem],
    after: &'a [ChecklistItem],
) -> Option<(usize, &'a ChecklistItem)> {
    if after.len() != before.len() + 1 {
        return None;
    }
    (0..after.len()).find_map(|index| {
        let matches = before[..index] == after[..index]
            && before[index..] == after[index.saturating_add(1)..];
        matches.then_some((index, &after[index]))
    })
}

fn single_removal<'a>(
    before: &'a [ChecklistItem],
    after: &[ChecklistItem],
) -> Option<(usize, &'a ChecklistItem)> {
    if before.len() != after.len() + 1 {
        return None;
    }
    (0..before.len()).find_map(|index| {
        let matches = before[..index] == after[..index]
            && before[index.saturating_add(1)..] == after[index..];
        matches.then_some((index, &before[index]))
    })
}

fn changed(
    events: &mut Vec<TaskHistoryEvent>,
    at: DateTime<Utc>,
    field: &str,
    from: &str,
    to: &str,
) {
    events.push(TaskHistoryEvent {
        at,
        kind: TaskHistoryKind::Changed {
            field: field.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
        },
    });
}

fn added(events: &mut Vec<TaskHistoryEvent>, at: DateTime<Utc>, field: &str, value: &str) {
    events.push(TaskHistoryEvent {
        at,
        kind: TaskHistoryKind::Added {
            field: field.to_owned(),
            value: value.to_owned(),
        },
    });
}

fn removed(events: &mut Vec<TaskHistoryEvent>, at: DateTime<Utc>, field: &str, value: &str) {
    events.push(TaskHistoryEvent {
        at,
        kind: TaskHistoryKind::Removed {
            field: field.to_owned(),
            value: value.to_owned(),
        },
    });
}

fn task_locations(board: &Board) -> impl Iterator<Item = TaskLocation<'_>> {
    board.columns.iter().flat_map(|column| {
        column
            .tasks
            .iter()
            .enumerate()
            .map(move |(index, task)| TaskLocation {
                column_id: column.id,
                column_title: &column.title,
                position: index + 1,
                task,
            })
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    #[test]
    fn checklist_status_changes_have_concise_descriptions() {
        let field = "checklist status for \"Run tests\"";
        assert_eq!(
            describe_checklist_status_change(field, "complete").as_deref(),
            Some("Checked Run tests")
        );
        assert_eq!(
            describe_checklist_status_change(field, "incomplete").as_deref(),
            Some("Unchecked Run tests")
        );
        assert_eq!(describe_checklist_status_change("title", "complete"), None);
    }

    #[test]
    fn derives_edits_tags_checklist_and_moves_from_snapshots() {
        let created = Utc.with_ymd_and_hms(2026, 7, 15, 10, 0, 0).unwrap();
        let edited = created + Duration::minutes(5);
        let moved = edited + Duration::minutes(5);
        let mut first = Board::default();
        first.add_task(0, "Initial title".into());
        first.columns[0].tasks[0].created_at = created;

        let mut second = first.clone();
        let task = &mut second.columns[0].tasks[0];
        task.title = "Updated title".into();
        task.description = "Useful context".into();
        task.tags.push("urgent".into());
        task.checklist.push(ChecklistItem {
            text: "Verify it".into(),
            completed: false,
            added_at: None,
            completed_at: None,
        });

        let mut third = second.clone();
        third.add_column("DONE".into()).unwrap();
        let task = third.columns[0].tasks.remove(0);
        third.columns[1].tasks.push(task);

        let histories = derive_task_history(&[(created, first), (edited, second), (moved, third)]);
        let events = &histories[&1];
        assert_eq!(events[0].kind, TaskHistoryKind::Created);
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            TaskHistoryKind::Changed { field, from, to }
                if field == "title" && from == "Initial title" && to == "Updated title"
        )));
        assert!(events.iter().any(|event| {
            matches!(&event.kind, TaskHistoryKind::TagAdded(tag) if tag == "urgent")
        }));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            TaskHistoryKind::Moved { from_column, to_column, .. }
                if from_column == "TODO" && to_column == "DONE"
        )));
    }

    #[test]
    fn detects_middle_checklist_removal_as_one_event() {
        let now = Utc::now();
        let items = |names: &[&str]| {
            names
                .iter()
                .map(|name| ChecklistItem {
                    text: (*name).into(),
                    completed: false,
                    added_at: None,
                    completed_at: None,
                })
                .collect::<Vec<_>>()
        };
        let mut first = Board::default();
        first.add_task(0, "Task".into());
        first.columns[0].tasks[0].checklist = items(&["one", "two", "three"]);
        let mut second = first.clone();
        second.columns[0].tasks[0].checklist = items(&["one", "three"]);

        let history = derive_task_history(&[(now, first), (now, second)]);
        let removed = history[&1]
            .iter()
            .filter(|event| matches!(event.kind, TaskHistoryKind::Removed { .. }))
            .count();
        assert_eq!(removed, 1);
    }

    #[test]
    fn scoped_movement_ignores_cards_shifted_as_a_side_effect() {
        let now = Utc::now();
        let mut before = Board::default();
        before.add_task(0, "one".into());
        before.add_task(0, "two".into());
        before.add_task(0, "three".into());
        let mut after = before.clone();
        let moved = after.columns[0].tasks.remove(0);
        after.columns[0].tasks.push(moved);

        let history = derive_board_changes_with_movement(
            Some(&before),
            &after,
            now,
            MovementTracking::Only(1),
        );
        assert!(matches!(history[&1][0].kind, TaskHistoryKind::Moved { .. }));
        assert!(!history.contains_key(&2));
        assert!(!history.contains_key(&3));
    }

    #[test]
    fn history_events_serialize_for_agent_cli_output() {
        let event = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::TagAdded("urgent".into()),
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["kind"]["type"], "tag_added");
        assert_eq!(json["kind"]["details"], "urgent");
    }
}
