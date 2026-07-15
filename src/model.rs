use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_COLUMNS: usize = 9;
pub const TAG_COLOR_PALETTE: [&str; 12] = [
    "#E06C75", "#D19A66", "#E5C07B", "#98C379", "#56B6C2", "#61AFEF", "#C678DD", "#FF6B9D",
    "#2EC4B6", "#7F7FFF", "#7F848E", "#A67C52",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Board {
    pub version: u32,
    pub columns: Vec<Column>,
    #[serde(default)]
    pub tags: Vec<TagDefinition>,
    pub next_column_id: u64,
    pub next_task_id: u64,
    #[serde(default = "default_next_tag_id")]
    pub next_tag_id: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Column {
    pub id: u64,
    pub title: String,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub checklist: Vec<ChecklistItem>,
    pub tags: Vec<String>,
    pub due_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChecklistItem {
    pub text: String,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

impl ChecklistItem {
    pub fn new(text: String) -> Self {
        Self {
            text,
            completed: false,
            added_at: Some(Utc::now()),
            completed_at: None,
        }
    }

    pub fn toggle(&mut self) {
        self.completed = !self.completed;
        self.completed_at = self.completed.then(Utc::now);
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TagDefinition {
    pub id: u64,
    pub name: String,
    pub color: String,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            version: 1,
            columns: vec![Column {
                id: 1,
                title: "TODO".into(),
                tasks: Vec::new(),
            }],
            tags: Vec::new(),
            next_column_id: 2,
            next_task_id: 1,
            next_tag_id: 1,
        }
    }
}

impl Board {
    pub fn validate_and_repair(&mut self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("unsupported board version {}", self.version));
        }
        if self.columns.is_empty() {
            return Err("the board must contain at least one column".into());
        }
        if self.columns.len() > MAX_COLUMNS {
            return Err(format!(
                "the board cannot contain more than {MAX_COLUMNS} columns"
            ));
        }

        let mut column_ids = HashSet::new();
        let mut task_ids = HashSet::new();
        let mut referenced_tags = Vec::new();
        for column in &mut self.columns {
            if column.id == 0 || !column_ids.insert(column.id) {
                return Err(format!("column id {} is zero or duplicated", column.id));
            }
            if column.title.trim().is_empty() {
                return Err(format!("column {} has an empty title", column.id));
            }
            for task in &mut column.tasks {
                if task.id == 0 || !task_ids.insert(task.id) {
                    return Err(format!("task id {} is zero or duplicated", task.id));
                }
                if task.title.trim().is_empty() {
                    return Err(format!("task {} has an empty title", task.id));
                }
                let mut unique_tags = HashSet::new();
                task.tags.retain_mut(|tag| {
                    *tag = tag.trim().trim_start_matches('#').to_owned();
                    !tag.is_empty() && unique_tags.insert(tag.to_ascii_lowercase())
                });
                referenced_tags.extend(task.tags.iter().cloned());
            }
        }

        let mut tag_ids = HashSet::new();
        let mut tag_names = HashSet::new();
        for tag in &mut self.tags {
            tag.name = tag.name.trim().trim_start_matches('#').to_owned();
            if tag.id == 0 || !tag_ids.insert(tag.id) {
                return Err(format!("tag id {} is zero or duplicated", tag.id));
            }
            if tag.name.is_empty() || !tag_names.insert(tag.name.to_ascii_lowercase()) {
                return Err(format!("tag name {:?} is empty or duplicated", tag.name));
            }
            tag.color = normalize_tag_color(&tag.color)
                .ok_or_else(|| format!("tag {} has invalid color {:?}", tag.id, tag.color))?;
        }

        let next_column_id = column_ids
            .into_iter()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "column IDs have been exhausted".to_owned())?;
        let next_task_id = task_ids
            .into_iter()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "task IDs have been exhausted".to_owned())?;
        let next_tag_id = tag_ids
            .into_iter()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "tag IDs have been exhausted".to_owned())?;
        self.next_column_id = self.next_column_id.max(next_column_id);
        self.next_task_id = self.next_task_id.max(next_task_id);
        self.next_tag_id = self.next_tag_id.max(next_tag_id);

        for name in referenced_tags {
            self.ensure_tag_definition(&name)?;
        }
        for column in &mut self.columns {
            for task in &mut column.tasks {
                for name in &mut task.tags {
                    if let Some(definition) = self
                        .tags
                        .iter()
                        .find(|tag| tag.name.eq_ignore_ascii_case(name))
                    {
                        *name = definition.name.clone();
                    }
                }
            }
        }
        Ok(())
    }

    pub fn add_column(&mut self, title: String) -> Result<usize, &'static str> {
        if self.columns.len() >= MAX_COLUMNS {
            return Err("a board can have at most 9 columns");
        }
        let index = self.columns.len();
        self.columns.push(Column {
            id: self.next_column_id,
            title,
            tasks: Vec::new(),
        });
        self.next_column_id += 1;
        Ok(index)
    }

    pub fn add_task(&mut self, column: usize, title: String) -> usize {
        let tasks = &mut self.columns[column].tasks;
        let index = tasks.len();
        tasks.push(Task {
            id: self.next_task_id,
            title,
            description: String::new(),
            checklist: Vec::new(),
            tags: Vec::new(),
            due_date: None,
            created_at: Utc::now(),
        });
        self.next_task_id += 1;
        index
    }

    pub fn delete_column(&mut self, index: usize) -> Result<usize, &'static str> {
        if index == 0 {
            return Err("the first column cannot be deleted because it has no prior column");
        }
        if index >= self.columns.len() {
            return Err("column does not exist");
        }
        let mut removed = self.columns.remove(index);
        let moved = removed.tasks.len();
        self.columns[index - 1].tasks.append(&mut removed.tasks);
        Ok(moved)
    }

    pub fn tag_by_name(&self, name: &str) -> Option<&TagDefinition> {
        self.tags
            .iter()
            .find(|tag| tag.name.eq_ignore_ascii_case(name))
    }

    pub fn create_tag(&mut self, name: &str, color: &str) -> Result<usize, String> {
        let name = name.trim().trim_start_matches('#');
        if name.is_empty() {
            return Err("tag name cannot be empty".into());
        }
        if self.tag_by_name(name).is_some() {
            return Err(format!("tag {name:?} already exists"));
        }
        let color = normalize_tag_color(color)
            .ok_or_else(|| format!("invalid tag color {color:?}; expected #RRGGBB"))?;
        let next_tag_id = self
            .next_tag_id
            .checked_add(1)
            .ok_or_else(|| "tag IDs have been exhausted".to_owned())?;
        let index = self.tags.len();
        self.tags.push(TagDefinition {
            id: self.next_tag_id,
            name: name.to_owned(),
            color,
        });
        self.next_tag_id = next_tag_id;
        Ok(index)
    }

    pub fn ensure_tag_definition(&mut self, name: &str) -> Result<usize, String> {
        if let Some(index) = self
            .tags
            .iter()
            .position(|tag| tag.name.eq_ignore_ascii_case(name))
        {
            return Ok(index);
        }
        let color = automatic_tag_color(name);
        self.create_tag(name, color)
    }
}

pub fn normalize_tag_color(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Some(value.to_ascii_uppercase())
    } else {
        None
    }
}

fn automatic_tag_color(name: &str) -> &'static str {
    let hash = name.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(usize::from(byte))
    });
    TAG_COLOR_PALETTE[hash % TAG_COLOR_PALETTE.len()]
}

const fn default_next_tag_id() -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_timestamps_are_backward_compatible_and_track_completion() {
        let legacy: ChecklistItem =
            serde_json::from_str(r#"{"text":"Legacy item","completed":true}"#).unwrap();
        assert_eq!(legacy.added_at, None);
        assert_eq!(legacy.completed_at, None);

        let mut item = ChecklistItem::new("New item".into());
        assert!(item.added_at.is_some());
        assert_eq!(item.completed_at, None);
        item.toggle();
        assert!(item.completed);
        assert!(item.completed_at.is_some());
        item.toggle();
        assert!(!item.completed);
        assert_eq!(item.completed_at, None);
    }

    #[test]
    fn repairs_stale_id_counters() {
        let mut board = Board::default();
        board.add_task(0, "one".into());
        board.next_column_id = 1;
        board.next_task_id = 1;

        board.validate_and_repair().unwrap();

        assert_eq!(board.next_column_id, 2);
        assert_eq!(board.next_task_id, 2);
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut board = Board::default();
        let duplicate = board.columns[0].clone();
        board.columns.push(duplicate);
        assert!(board.validate_and_repair().is_err());
    }

    #[test]
    fn migrates_string_tags_to_colored_definitions() {
        let mut board = Board::default();
        board.add_task(0, "tagged".into());
        board.columns[0].tasks[0].tags = vec!["#Release".into(), "release".into()];

        board.validate_and_repair().unwrap();

        assert_eq!(board.columns[0].tasks[0].tags, ["Release"]);
        let tag = board.tag_by_name("release").unwrap();
        assert!(normalize_tag_color(&tag.color).is_some());
    }

    #[test]
    fn normalizes_hex_tag_colors() {
        assert_eq!(normalize_tag_color("#a1b2c3"), Some("#A1B2C3".into()));
        assert_eq!(normalize_tag_color("blue"), None);
    }

    #[test]
    fn deleting_a_column_moves_its_tasks_to_the_prior_column() {
        let mut board = Board::default();
        board.add_column("DOING".into()).unwrap();
        board.add_task(0, "already there".into());
        let first_moved = board.add_task(1, "first moved".into());
        let second_moved = board.add_task(1, "second moved".into());
        let moved_ids = [
            board.columns[1].tasks[first_moved].id,
            board.columns[1].tasks[second_moved].id,
        ];

        assert_eq!(board.delete_column(1), Ok(2));

        assert_eq!(board.columns.len(), 1);
        assert_eq!(board.columns[0].tasks.len(), 3);
        assert_eq!(board.columns[0].tasks[1].id, moved_ids[0]);
        assert_eq!(board.columns[0].tasks[2].id, moved_ids[1]);
        assert!(board.delete_column(0).is_err());
    }
}
