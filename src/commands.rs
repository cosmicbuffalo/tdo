use std::path::Path;

use anyhow::{Result, bail};
use chrono::NaiveDate;
use serde::Serialize;

use crate::{
    config::{
        AppConfig, ChecklistCommand, Cli, ColumnCommand, Command, ConfigCommand, TagCommand,
        TaskCommand,
    },
    model::{
        Board, ChecklistItem, Column, TAG_COLOR_PALETTE, TagDefinition, Task, normalize_tag_color,
    },
    storage::Store,
};

pub fn run(
    cli: &Cli,
    config: &AppConfig,
    config_path: &Path,
    board: &mut Board,
    store: &Store,
) -> Result<()> {
    let Some(command) = &cli.command else {
        return Ok(());
    };
    match command {
        Command::List => print_board(board, cli.json)?,
        Command::Column(args) => run_column(&args.command, board, store, cli.json)?,
        Command::Task(args) => run_task(&args.command, board, store, cli.json)?,
        Command::Checklist(args) => run_checklist(&args.command, board, store, cli.json)?,
        Command::Tag(args) => run_tag(&args.command, board, store, cli.json)?,
        Command::Config(args) => match args.command {
            ConfigCommand::Show if cli.json => {
                println!("{}", serde_json::to_string_pretty(config)?);
            }
            ConfigCommand::Show => {
                print!("{}", toml::to_string_pretty(config)?);
            }
            ConfigCommand::Path => println!("{}", config_path.display()),
        },
    }
    store.maybe_push()?;
    Ok(())
}

fn run_column(command: &ColumnCommand, board: &mut Board, store: &Store, json: bool) -> Result<()> {
    match command {
        ColumnCommand::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&board.columns)?);
            } else {
                for column in &board.columns {
                    println!(
                        "{}\t{}\t{} tasks",
                        column.id,
                        column.title,
                        column.tasks.len()
                    );
                }
            }
        }
        ColumnCommand::Show { column_id } => {
            let (_, column) = find_column(board, *column_id)?;
            print_value(column, json)?;
        }
        ColumnCommand::Add { name } => {
            let name = required(name, "column name")?;
            let index = board
                .add_column(name.to_owned())
                .map_err(anyhow::Error::msg)?;
            store.save(board, &format!("Add column {name}"))?;
            print_value(&board.columns[index], json)?;
        }
        ColumnCommand::Rename { column_id, name } => {
            let name = required(name, "column name")?.to_owned();
            let index = find_column(board, *column_id)?.0;
            board.columns[index].title = name.clone();
            store.save(board, &format!("Rename column {column_id} to {name}"))?;
            print_value(&board.columns[index], json)?;
        }
        ColumnCommand::Delete { column_id } => {
            let index = find_column(board, *column_id)?.0;
            if index == 0 {
                bail!("the first column cannot be deleted because it has no prior column");
            }
            let title = board.columns[index].title.clone();
            let moved_to_column_id = board.columns[index - 1].id;
            let moved_task_count = board.delete_column(index).map_err(anyhow::Error::msg)?;
            store.save(
                board,
                &format!(
                    "Delete column {column_id} ({title}); move {moved_task_count} tasks to prior column"
                ),
            )?;
            print_value(
                &DeletedColumn {
                    column_id: *column_id,
                    title,
                    moved_task_count,
                    moved_to_column_id,
                },
                json,
            )?;
        }
    }
    Ok(())
}

fn run_task(command: &TaskCommand, board: &mut Board, store: &Store, json: bool) -> Result<()> {
    match command {
        TaskCommand::List { column } => {
            if let Some(id) = column {
                let (_, column) = find_column(board, *id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&column.tasks)?);
                } else {
                    for task in &column.tasks {
                        print_task_line(task, column);
                    }
                }
            } else if json {
                let tasks: Vec<_> = board
                    .columns
                    .iter()
                    .flat_map(|column| {
                        column.tasks.iter().map(move |task| TaskWithColumn {
                            column_id: column.id,
                            task,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else {
                for column in &board.columns {
                    for task in &column.tasks {
                        print_task_line(task, column);
                    }
                }
            }
        }
        TaskCommand::Show { task_id } => {
            let (_, _, task) = find_task(board, *task_id)?;
            print_value(task, json)?;
        }
        TaskCommand::Add {
            column_id,
            title,
            description,
            tags,
            due,
        } => {
            let title = required(title, "task title")?.to_owned();
            let column_index = find_column(board, *column_id)?.0;
            let due_date = due.as_deref().map(parse_date).transpose()?;
            let mut tags = normalize_tags(tags.clone());
            ensure_tag_definitions(board, &mut tags)?;
            let task_index = board.add_task(column_index, title);
            let task = &mut board.columns[column_index].tasks[task_index];
            task.description = description.clone();
            task.tags = tags;
            task.due_date = due_date;
            let task_id = task.id;
            store.save(board, &format!("Add task {task_id}"))?;
            print_value(&board.columns[column_index].tasks[task_index], json)?;
        }
        TaskCommand::Edit {
            task_id,
            title,
            description,
            tags,
            clear_tags,
            due,
            clear_due,
        } => {
            let updated_tags = if *clear_tags {
                Some(Vec::new())
            } else if !tags.is_empty() {
                let mut tags = normalize_tags(tags.clone());
                ensure_tag_definitions(board, &mut tags)?;
                Some(tags)
            } else {
                None
            };
            let (column_index, task_index, _) = find_task(board, *task_id)?;
            let task = &mut board.columns[column_index].tasks[task_index];
            if let Some(title) = title {
                task.title = required(title, "task title")?.to_owned();
            }
            if let Some(description) = description {
                task.description = description.clone();
            }
            if let Some(tags) = updated_tags {
                task.tags = tags;
            }
            if *clear_due {
                task.due_date = None;
            } else if let Some(due) = due {
                task.due_date = Some(parse_date(due)?);
            }
            store.save(board, &format!("Edit task {task_id}"))?;
            print_value(&board.columns[column_index].tasks[task_index], json)?;
        }
        TaskCommand::Move {
            task_id,
            column,
            position,
        } => {
            let (source_column, source_task, _) = find_task(board, *task_id)?;
            let target_column = find_column(board, *column)?.0;
            let task = board.columns[source_column].tasks.remove(source_task);
            let target_len = board.columns[target_column].tasks.len();
            let target_index = match position {
                Some(0) => bail!("position is one-based and must be at least 1"),
                Some(position) => (position - 1).min(target_len),
                None => target_len,
            };
            board.columns[target_column]
                .tasks
                .insert(target_index, task);
            store.save(board, &format!("Move task {task_id} to column {column}"))?;
            print_value(&board.columns[target_column].tasks[target_index], json)?;
        }
        TaskCommand::Delete { task_id } => {
            let (column, task, _) = find_task(board, *task_id)?;
            let removed = board.columns[column].tasks.remove(task);
            store.save(board, &format!("Delete task {task_id} ({})", removed.title))?;
            print_value(&removed, json)?;
        }
    }
    Ok(())
}

fn run_checklist(
    command: &ChecklistCommand,
    board: &mut Board,
    store: &Store,
    json: bool,
) -> Result<()> {
    let (task_id, message) = match command {
        ChecklistCommand::Add { task_id, text } => {
            let text = required(text, "checklist text")?.to_owned();
            let (column, task, _) = find_task(board, *task_id)?;
            board.columns[column].tasks[task]
                .checklist
                .push(ChecklistItem {
                    text,
                    completed: false,
                });
            (*task_id, format!("Add checklist item to task {task_id}"))
        }
        ChecklistCommand::Edit {
            task_id,
            item,
            text,
        } => {
            let text = required(text, "checklist text")?.to_owned();
            let checklist_item = find_checklist_item_mut(board, *task_id, *item)?;
            checklist_item.text = text;
            (*task_id, format!("Edit checklist item on task {task_id}"))
        }
        ChecklistCommand::Toggle { task_id, item } => {
            let checklist_item = find_checklist_item_mut(board, *task_id, *item)?;
            checklist_item.completed = !checklist_item.completed;
            (*task_id, format!("Toggle checklist item on task {task_id}"))
        }
        ChecklistCommand::Remove { task_id, item } => {
            if *item == 0 {
                bail!("checklist item is one-based and must be at least 1");
            }
            let (column, task, _) = find_task(board, *task_id)?;
            let checklist = &mut board.columns[column].tasks[task].checklist;
            if *item > checklist.len() {
                bail!("checklist item {item} does not exist");
            }
            checklist.remove(item - 1);
            (
                *task_id,
                format!("Remove checklist item from task {task_id}"),
            )
        }
    };
    store.save(board, &message)?;
    let (_, _, task) = find_task(board, task_id)?;
    print_value(task, json)
}

fn run_tag(command: &TagCommand, board: &mut Board, store: &Store, json: bool) -> Result<()> {
    match command {
        TagCommand::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&board.tags)?);
            } else {
                for tag in &board.tags {
                    println!("{}\t{}\t{}", tag.id, tag.name, tag.color);
                }
            }
        }
        TagCommand::Show { tag_id } => print_value(find_tag(board, *tag_id)?.1, json)?,
        TagCommand::Create { name, color } => {
            let name = required(name, "tag name")?;
            let color = color.clone().unwrap_or_else(|| {
                TAG_COLOR_PALETTE[board.next_tag_id as usize % TAG_COLOR_PALETTE.len()].into()
            });
            let index = board.create_tag(name, &color).map_err(anyhow::Error::msg)?;
            store.save(board, &format!("Create tag {name}"))?;
            print_value(&board.tags[index], json)?;
        }
        TagCommand::SetColor { tag_id, color } => {
            let color = normalize_tag_color(color)
                .ok_or_else(|| anyhow::anyhow!("invalid tag color {color:?}; expected #RRGGBB"))?;
            let index = find_tag(board, *tag_id)?.0;
            board.tags[index].color = color;
            store.save(board, &format!("Change color for tag {tag_id}"))?;
            print_value(&board.tags[index], json)?;
        }
    }
    Ok(())
}

fn find_column(board: &Board, id: u64) -> Result<(usize, &Column)> {
    board
        .columns
        .iter()
        .enumerate()
        .find(|(_, column)| column.id == id)
        .ok_or_else(|| anyhow::anyhow!("column {id} does not exist"))
}

fn find_task(board: &Board, id: u64) -> Result<(usize, usize, &Task)> {
    board
        .columns
        .iter()
        .enumerate()
        .find_map(|(column_index, column)| {
            column
                .tasks
                .iter()
                .enumerate()
                .find(|(_, task)| task.id == id)
                .map(|(task_index, task)| (column_index, task_index, task))
        })
        .ok_or_else(|| anyhow::anyhow!("task {id} does not exist"))
}

fn find_tag(board: &Board, id: u64) -> Result<(usize, &TagDefinition)> {
    board
        .tags
        .iter()
        .enumerate()
        .find(|(_, tag)| tag.id == id)
        .ok_or_else(|| anyhow::anyhow!("tag {id} does not exist"))
}

fn find_checklist_item_mut(
    board: &mut Board,
    task_id: u64,
    item: usize,
) -> Result<&mut ChecklistItem> {
    if item == 0 {
        bail!("checklist item is one-based and must be at least 1");
    }
    let (column, task, _) = find_task(board, task_id)?;
    board.columns[column].tasks[task]
        .checklist
        .get_mut(item - 1)
        .ok_or_else(|| anyhow::anyhow!("checklist item {item} does not exist"))
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| anyhow::anyhow!("invalid date {value:?}; expected YYYY-MM-DD"))
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{field} cannot be empty");
    }
    Ok(value)
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = tag.trim().trim_start_matches('#').to_owned();
        if !tag.is_empty()
            && !normalized
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&tag))
        {
            normalized.push(tag);
        }
    }
    normalized
}

fn ensure_tag_definitions(board: &mut Board, tags: &mut [String]) -> Result<()> {
    for tag in tags {
        board
            .ensure_tag_definition(tag)
            .map_err(anyhow::Error::msg)?;
        *tag = board.tag_by_name(tag).unwrap().name.clone();
    }
    Ok(())
}

fn print_board(board: &Board, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(board)?);
    } else {
        for column in &board.columns {
            println!(
                "[{}] {} ({} tasks)",
                column.id,
                column.title,
                column.tasks.len()
            );
            for task in &column.tasks {
                println!("  [{}] {}", task.id, task.title);
            }
        }
    }
    Ok(())
}

fn print_value<T: Serialize + std::fmt::Debug>(value: &T, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{value:#?}");
    }
    Ok(())
}

fn print_task_line(task: &Task, column: &Column) {
    println!(
        "{}\t{}\tcolumn {} ({})",
        task.id, task.title, column.id, column.title
    );
}

#[derive(Serialize)]
struct TaskWithColumn<'a> {
    column_id: u64,
    #[serde(flatten)]
    task: &'a Task,
}

#[derive(Debug, Serialize)]
struct DeletedColumn {
    column_id: u64,
    title: String,
    moved_task_count: usize,
    moved_to_column_id: u64,
}
