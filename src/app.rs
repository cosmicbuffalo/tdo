use chrono::{Datelike, Duration, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
use tui_textarea::{CursorMove, TextArea};

use crate::history::TaskHistory;
use crate::model::{Board, ChecklistItem, MAX_COLUMNS, TAG_COLOR_PALETTE};

#[derive(Debug, Eq, PartialEq)]
pub enum Action {
    None,
    Quit,
    Save(String),
    EditTextExternally,
}

#[derive(Clone, Debug)]
pub enum Mode {
    Board,
    TaskDetails { cursor: usize },
    ColumnDetails { cursor: usize },
    Input(InputState),
    DatePicker(DatePickerState),
    TagPicker(TagPickerState),
    NewTag(NewTagState),
    Moving(MoveState),
    ConfirmDelete(DeleteConfirmation),
    Help { return_to: Box<Mode> },
}

#[derive(Clone, Debug)]
pub struct DeleteConfirmation {
    pub target: DeleteTarget,
    pub choice: DeleteChoice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteTarget {
    Column { index: usize },
    Task { column: usize, task: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteChoice {
    Cancel,
    Delete,
}

#[derive(Clone, Debug)]
pub struct InputState {
    pub kind: InputKind,
    pub editor: TextEditor,
    return_to: ReturnTo,
}

#[derive(Clone, Debug)]
pub struct TextEditor {
    area: TextArea<'static>,
}

#[derive(Clone, Debug)]
pub struct DatePickerState {
    pub selected: NaiveDate,
    original: Option<NaiveDate>,
    return_to: ReturnTo,
}

#[derive(Clone, Debug)]
pub struct TagPickerState {
    pub row: TagPickerRow,
    pub index: usize,
    return_to: ReturnTo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TagPickerRow {
    Current,
    Available,
}

#[derive(Clone, Debug)]
pub struct NewTagState {
    pub name: TextEditor,
    pub color_index: usize,
    pub field: NewTagField,
    picker: TagPickerState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewTagField {
    Name,
    Color,
}

#[derive(Clone, Debug)]
pub enum InputKind {
    AddColumn,
    RenameColumn,
    AddTask,
    TaskTitle,
    TaskDescription,
    AddChecklistItem,
    EditChecklistItem(usize),
}

#[derive(Clone, Debug)]
enum ReturnTo {
    Board,
    TaskDetails(usize),
    ColumnDetails(usize),
}

#[derive(Clone, Debug)]
pub struct MoveState {
    snapshot: Board,
    origin_column: usize,
    origin_task: usize,
}

impl TextEditor {
    fn new(text: impl AsRef<str>) -> Self {
        let mut editor = Self {
            area: TextArea::default(),
        };
        editor.set_text(text.as_ref());
        editor
    }

    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn lines(&self) -> &[String] {
        self.area.lines()
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.area.cursor()
    }

    fn input(&mut self, key: KeyEvent) {
        self.area.input(key);
    }

    fn set_text(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n");
        self.area = TextArea::new(normalized.split('\n').map(str::to_owned).collect());
        self.area.move_cursor(CursorMove::Bottom);
        self.area.move_cursor(CursorMove::End);
    }
}

pub struct App {
    pub board: Board,
    pub task_history: TaskHistory,
    pub task_history_earlier: HashMap<u64, usize>,
    pub selected_column: usize,
    pub selected_task: Option<usize>,
    column_cursors: HashMap<u64, Option<usize>>,
    column_scrolls: HashMap<u64, usize>,
    column_scroll_follows_cursor: HashMap<u64, bool>,
    pub task_detail_scroll: usize,
    pub task_detail_follow_cursor: bool,
    pub mode: Mode,
    pub status: Option<String>,
}

impl App {
    pub fn new(board: Board) -> Self {
        Self {
            board,
            task_history: TaskHistory::new(),
            task_history_earlier: HashMap::new(),
            selected_column: 0,
            selected_task: None,
            column_cursors: HashMap::new(),
            column_scrolls: HashMap::new(),
            column_scroll_follows_cursor: HashMap::new(),
            task_detail_scroll: 0,
            task_detail_follow_cursor: true,
            mode: Mode::Board,
            status: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        self.status = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        let editing_text = self.is_text_editor_active();
        if key.code == KeyCode::Char('q') && !editing_text {
            return if self.close_floating_mode() {
                Action::None
            } else {
                Action::Quit
            };
        }
        if let Mode::Help { return_to } = &self.mode {
            if key.code == KeyCode::Esc {
                self.mode = *return_to.clone();
            }
            return Action::None;
        }
        let control_slash = (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char('/') | KeyCode::Char('_') | KeyCode::Char('7')
            ))
            || key.code == KeyCode::Char('\u{1f}');
        if control_slash || (key.code == KeyCode::Char('?') && !editing_text) {
            self.mode = Mode::Help {
                return_to: Box::new(self.mode.clone()),
            };
            return Action::None;
        }
        match self.mode.clone() {
            Mode::Board => self.handle_board_key(key),
            Mode::TaskDetails { cursor } => self.handle_task_details_key(key, cursor),
            Mode::ColumnDetails { cursor } => self.handle_column_details_key(key, cursor),
            Mode::Input(_) => self.handle_input_key(key),
            Mode::DatePicker(_) => self.handle_date_picker_key(key),
            Mode::TagPicker(_) => self.handle_tag_picker_key(key),
            Mode::NewTag(_) => self.handle_new_tag_key(key),
            Mode::Moving(_) => self.handle_move_key(key),
            Mode::ConfirmDelete(state) => self.handle_delete_confirmation_key(key, state),
            Mode::Help { .. } => unreachable!(),
        }
    }

    pub fn report_saved(&mut self, message: &str) {
        self.status = Some(format!("saved + committed: {message}"));
    }

    pub fn report_error(&mut self, error: &anyhow::Error) {
        self.status = Some(format!("error: {error:#}"));
    }

    pub fn can_reload_external_changes(&self) -> bool {
        matches!(
            self.mode,
            Mode::Board | Mode::TaskDetails { .. } | Mode::ColumnDetails { .. }
        )
    }

    pub fn replace_board_from_external_change(&mut self, board: Board) {
        let selected_column_id = self
            .board
            .columns
            .get(self.selected_column)
            .map(|column| column.id);
        let selected_task_id = self.selected_task.and_then(|task| {
            self.board
                .columns
                .get(self.selected_column)
                .and_then(|column| column.tasks.get(task))
                .map(|task| task.id)
        });
        let previous_mode = self.mode.clone();
        self.board = board;

        let selected_task = selected_task_id.and_then(|task_id| {
            self.board
                .columns
                .iter()
                .enumerate()
                .find_map(|(column_index, column)| {
                    column
                        .tasks
                        .iter()
                        .position(|task| task.id == task_id)
                        .map(|task_index| (column_index, task_index))
                })
        });
        if let Some((column, task)) = selected_task {
            self.selected_column = column;
            self.selected_task = Some(task);
            self.remember_current_column_cursor();
            self.follow_column_cursor(column);
        } else {
            self.selected_column = selected_column_id
                .and_then(|column_id| {
                    self.board
                        .columns
                        .iter()
                        .position(|column| column.id == column_id)
                })
                .unwrap_or(0)
                .min(self.board.columns.len().saturating_sub(1));
            self.selected_task = None;
            self.remember_current_column_cursor();
        }

        self.mode = match previous_mode {
            Mode::TaskDetails { cursor } if self.selected_task.is_some() => Mode::TaskDetails {
                cursor: cursor.min(self.task_detail_count().saturating_sub(1)),
            },
            Mode::ColumnDetails { cursor } => Mode::ColumnDetails { cursor },
            _ => Mode::Board,
        };
        self.task_history.clear();
        self.task_history_earlier.clear();
        self.task_detail_follow_cursor = true;
        self.status = Some("reloaded external board changes".into());
    }

    pub fn select_target(&mut self, column: usize, task: Option<usize>) {
        if !matches!(self.mode, Mode::Board) || column >= self.board.columns.len() {
            return;
        }
        self.remember_current_column_cursor();
        self.selected_column = column;
        self.selected_task = task.filter(|index| *index < self.current_column().tasks.len());
        self.remember_current_column_cursor();
        if self.selected_task.is_some() {
            self.follow_column_cursor(column);
        }
    }

    fn column_cursor(&self, column: usize) -> Option<usize> {
        self.board
            .columns
            .get(column)
            .and_then(|column| self.column_cursors.get(&column.id))
            .copied()
            .flatten()
    }

    fn remember_current_column_cursor(&mut self) {
        let Some(column) = self.board.columns.get(self.selected_column) else {
            return;
        };
        let cursor = self.selected_task.filter(|task| *task < column.tasks.len());
        self.column_cursors.insert(column.id, cursor);
    }

    pub fn column_scroll(&self, column: usize) -> usize {
        self.board
            .columns
            .get(column)
            .and_then(|column| self.column_scrolls.get(&column.id))
            .copied()
            .unwrap_or(0)
    }

    pub fn set_column_scroll(&mut self, column: usize, start: usize) {
        let Some(column) = self.board.columns.get(column) else {
            return;
        };
        let max_start = column.tasks.len().saturating_sub(1);
        self.column_scrolls.insert(column.id, start.min(max_start));
    }

    pub fn scroll_column(&mut self, column: usize, tasks: isize) {
        let Some(column_id) = self.board.columns.get(column).map(|column| column.id) else {
            return;
        };
        let current = self.column_scroll(column);
        let start = if tasks < 0 {
            current.saturating_sub(tasks.unsigned_abs())
        } else {
            current.saturating_add(tasks as usize)
        };
        self.set_column_scroll(column, start);
        self.column_scroll_follows_cursor.insert(column_id, false);
    }

    pub fn column_scroll_follows_cursor(&self, column: usize) -> bool {
        self.board
            .columns
            .get(column)
            .and_then(|column| self.column_scroll_follows_cursor.get(&column.id))
            .copied()
            .unwrap_or(true)
    }

    fn follow_column_cursor(&mut self, column: usize) {
        if let Some(column) = self.board.columns.get(column) {
            self.column_scroll_follows_cursor.insert(column.id, true);
        }
    }

    pub fn open_selected_task_details(&mut self) {
        if matches!(self.mode, Mode::Board) && self.selected_task.is_some() {
            self.task_detail_scroll = 0;
            self.task_detail_follow_cursor = true;
            self.mode = Mode::TaskDetails { cursor: 0 };
        }
    }

    pub fn close_task_details(&mut self) {
        if matches!(self.mode, Mode::TaskDetails { .. }) {
            self.mode = Mode::Board;
        }
    }

    pub fn scroll_task_details(&mut self, lines: isize) {
        if !matches!(self.mode, Mode::TaskDetails { .. }) {
            return;
        }
        self.task_detail_follow_cursor = false;
        self.task_detail_scroll = if lines < 0 {
            self.task_detail_scroll.saturating_sub(lines.unsigned_abs())
        } else {
            self.task_detail_scroll.saturating_add(lines as usize)
        };
    }

    pub fn click_checklist_item(&mut self, index: usize) -> Action {
        if index >= self.current_task().checklist.len() {
            return Action::None;
        }
        self.mode = Mode::TaskDetails { cursor: index + 4 };
        self.task_detail_follow_cursor = true;
        self.toggle_checklist_item(index)
    }

    pub fn toggle_checklist_item(&mut self, index: usize) -> Action {
        if !matches!(self.mode, Mode::TaskDetails { .. }) {
            return Action::None;
        }
        let Some(item) = self.current_task_mut().checklist.get_mut(index) else {
            return Action::None;
        };
        item.toggle();
        Action::Save("Toggle checklist item".into())
    }

    fn handle_board_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('C') => {
                if self.board.columns.len() >= MAX_COLUMNS {
                    self.status = Some("the 9-column limit has been reached".into());
                } else {
                    self.open_input(InputKind::AddColumn, "", ReturnTo::Board);
                }
            }
            KeyCode::Char('a') => self.open_input(InputKind::AddTask, "", ReturnTo::Board),
            KeyCode::Char('r') if self.selected_task.is_none() => {
                let title = self.current_column().title.clone();
                self.open_input(InputKind::RenameColumn, title, ReturnTo::Board);
            }
            KeyCode::Char('m') if self.selected_task.is_some() => self.begin_move(),
            KeyCode::Char('D') => self.begin_delete(),
            KeyCode::Enter => {
                if self.selected_task.is_some() {
                    self.open_selected_task_details();
                } else {
                    self.mode = Mode::ColumnDetails { cursor: 0 };
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.select_up(),
            KeyCode::Down | KeyCode::Char('j') => self.select_down(),
            KeyCode::Left | KeyCode::Char('h') => self.select_column(-1),
            KeyCode::Right | KeyCode::Char('l') => self.select_column(1),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit.to_digit(10).unwrap() as usize - 1;
                self.jump_to_column(index);
            }
            _ => {}
        }
        Action::None
    }

    fn handle_delete_confirmation_key(
        &mut self,
        key: KeyEvent,
        state: DeleteConfirmation,
    ) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                self.mode = Mode::Board;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.mode = Mode::ConfirmDelete(DeleteConfirmation {
                    choice: DeleteChoice::Cancel,
                    ..state
                });
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.mode = Mode::ConfirmDelete(DeleteConfirmation {
                    choice: DeleteChoice::Delete,
                    ..state
                });
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.mode = Mode::ConfirmDelete(DeleteConfirmation {
                    choice: match state.choice {
                        DeleteChoice::Cancel => DeleteChoice::Delete,
                        DeleteChoice::Delete => DeleteChoice::Cancel,
                    },
                    ..state
                });
            }
            KeyCode::Enter if state.choice == DeleteChoice::Cancel => {
                self.mode = Mode::Board;
            }
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                return self.confirm_delete(state.target);
            }
            _ => {}
        }
        Action::None
    }

    fn handle_task_details_key(&mut self, key: KeyEvent, cursor: usize) -> Action {
        let count = self.task_detail_count();
        match key.code {
            KeyCode::Esc => self.mode = Mode::Board,
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                self.task_detail_follow_cursor = true;
                self.mode = Mode::TaskDetails {
                    cursor: cursor.saturating_sub(1),
                };
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                self.task_detail_follow_cursor = true;
                self.mode = Mode::TaskDetails {
                    cursor: (cursor + 1).min(count.saturating_sub(1)),
                };
            }
            KeyCode::Char('a') => self.open_input(
                InputKind::AddChecklistItem,
                "",
                ReturnTo::TaskDetails(cursor),
            ),
            KeyCode::Char('d') => {
                if let Some(index) = self.checklist_index_at(cursor) {
                    self.current_task_mut().checklist.remove(index);
                    let new_count = self.task_detail_count();
                    self.mode = Mode::TaskDetails {
                        cursor: cursor.min(new_count.saturating_sub(1)),
                    };
                    return Action::Save("Remove checklist item".into());
                }
            }
            KeyCode::Char(' ') => {
                if let Some(index) = self.checklist_index_at(cursor) {
                    return self.toggle_checklist_item(index);
                }
            }
            KeyCode::Enter => {
                if let Some(index) = self.checklist_index_at(cursor) {
                    return self.toggle_checklist_item(index);
                }
                self.edit_task_field(cursor);
            }
            KeyCode::Char('e') => self.edit_task_field(cursor),
            _ => {}
        }
        Action::None
    }

    fn handle_column_details_key(&mut self, key: KeyEvent, cursor: usize) -> Action {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Board,
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                self.mode = Mode::ColumnDetails { cursor: 0 };
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                self.mode = Mode::ColumnDetails { cursor: 1 };
            }
            KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char('r') if cursor == 0 => {
                self.open_input(
                    InputKind::RenameColumn,
                    self.current_column().title.clone(),
                    ReturnTo::ColumnDetails(cursor),
                );
            }
            _ => {}
        }
        Action::None
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Mode::Input(input) = self.mode.clone() {
                    self.restore_return_mode(input.return_to);
                }
            }
            KeyCode::Enter => return self.submit_input(),
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::EditTextExternally;
            }
            _ => {
                if let Mode::Input(input) = &mut self.mode {
                    input.editor.input(key);
                }
            }
        }
        Action::None
    }

    fn handle_date_picker_key(&mut self, key: KeyEvent) -> Action {
        let Mode::DatePicker(picker) = self.mode.clone() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.restore_return_mode(picker.return_to),
            KeyCode::Enter => {
                self.current_task_mut().due_date = Some(picker.selected);
                self.restore_return_mode(picker.return_to);
                if picker.original != Some(picker.selected) {
                    return Action::Save("Edit task due date".into());
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                self.current_task_mut().due_date = None;
                self.restore_return_mode(picker.return_to);
                if picker.original.is_some() {
                    return Action::Save("Clear task due date".into());
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.move_picker_days(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_picker_days(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_picker_days(-7),
            KeyCode::Down | KeyCode::Char('j') => self.move_picker_days(7),
            KeyCode::PageUp => self.move_picker_months(-1),
            KeyCode::PageDown => self.move_picker_months(1),
            KeyCode::Char('t') => {
                if let Mode::DatePicker(picker) = &mut self.mode {
                    picker.selected = Local::now().date_naive();
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_tag_picker_key(&mut self, key: KeyEvent) -> Action {
        let Mode::TagPicker(mut picker) = self.mode.clone() else {
            return Action::None;
        };
        let current_count = self.current_task().tags.len();
        let available = self.available_tag_names();
        let row_count = match picker.row {
            TagPickerRow::Current => current_count,
            TagPickerRow::Available => available.len() + 1,
        };
        match key.code {
            KeyCode::Esc => self.restore_return_mode(picker.return_to),
            KeyCode::Left | KeyCode::Char('h') => {
                picker.index = picker.index.saturating_sub(1);
                self.mode = Mode::TagPicker(picker);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                picker.index = (picker.index + 1).min(row_count.saturating_sub(1));
                self.mode = Mode::TagPicker(picker);
            }
            KeyCode::Up | KeyCode::Char('k') if current_count > 0 => {
                picker.row = TagPickerRow::Current;
                picker.index = picker.index.min(current_count - 1);
                self.mode = Mode::TagPicker(picker);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.row = TagPickerRow::Available;
                picker.index = picker.index.min(available.len());
                self.mode = Mode::TagPicker(picker);
            }
            KeyCode::Enter => match picker.row {
                TagPickerRow::Current if current_count > 0 => {
                    let index = picker.index.min(current_count - 1);
                    let name = self.current_task_mut().tags.remove(index);
                    let remaining = self.current_task().tags.len();
                    if remaining == 0 {
                        picker.row = TagPickerRow::Available;
                        picker.index = 0;
                    } else {
                        picker.index = index.min(remaining - 1);
                    }
                    self.mode = Mode::TagPicker(picker);
                    return Action::Save(format!("Remove tag {name}"));
                }
                TagPickerRow::Available if picker.index < available.len() => {
                    let name = available[picker.index].clone();
                    self.current_task_mut().tags.push(name.clone());
                    let remaining_available = available.len().saturating_sub(1);
                    picker.index = picker.index.min(remaining_available);
                    self.mode = Mode::TagPicker(picker);
                    return Action::Save(format!("Add tag {name}"));
                }
                TagPickerRow::Available => self.open_new_tag(picker),
                TagPickerRow::Current => {
                    picker.row = TagPickerRow::Available;
                    picker.index = 0;
                    self.mode = Mode::TagPicker(picker);
                }
            },
            _ => {}
        }
        Action::None
    }

    fn handle_new_tag_key(&mut self, key: KeyEvent) -> Action {
        let Mode::NewTag(mut state) = self.mode.clone() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::TagPicker(state.picker),
            KeyCode::Char('g')
                if state.field == NewTagField::Name
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.mode = Mode::NewTag(state);
                return Action::EditTextExternally;
            }
            KeyCode::Tab => {
                state.field = match state.field {
                    NewTagField::Name => NewTagField::Color,
                    NewTagField::Color => NewTagField::Name,
                };
                self.mode = Mode::NewTag(state);
            }
            KeyCode::BackTab => {
                state.field = match state.field {
                    NewTagField::Name => NewTagField::Color,
                    NewTagField::Color => NewTagField::Name,
                };
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Up if state.field == NewTagField::Color => {
                state.field = NewTagField::Name;
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Down if state.field == NewTagField::Name => {
                state.field = NewTagField::Color;
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Left | KeyCode::Char('h') if state.field == NewTagField::Color => {
                state.color_index = state.color_index.saturating_sub(1);
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Right | KeyCode::Char('l') if state.field == NewTagField::Color => {
                state.color_index =
                    (state.color_index + 1).min(TAG_COLOR_PALETTE.len().saturating_sub(1));
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Enter if state.field == NewTagField::Name => {
                state.field = NewTagField::Color;
                self.mode = Mode::NewTag(state);
            }
            KeyCode::Enter => return self.submit_new_tag(state),
            _ if state.field == NewTagField::Name => {
                state.name.input(key);
                self.mode = Mode::NewTag(state);
            }
            _ => {}
        }
        Action::None
    }

    fn handle_move_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                let Mode::Moving(state) = self.mode.clone() else {
                    unreachable!()
                };
                self.board = state.snapshot;
                self.selected_column = state.origin_column;
                self.selected_task = Some(state.origin_task);
                self.remember_current_column_cursor();
                self.follow_column_cursor(state.origin_column);
                self.mode = Mode::Board;
                self.status = Some("move cancelled".into());
            }
            KeyCode::Enter | KeyCode::Char('m') => {
                let task_id = self.current_task().id;
                self.mode = Mode::Board;
                return Action::Save(format!("Move task {task_id}"));
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_task_vertically(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_task_vertically(1),
            KeyCode::Left | KeyCode::Char('h') => self.move_task_horizontally(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_task_horizontally(1),
            _ => {}
        }
        Action::None
    }

    fn submit_input(&mut self) -> Action {
        let Mode::Input(input) = self.mode.clone() else {
            return Action::None;
        };
        let text = input.editor.text().trim().to_owned();
        match input.kind {
            InputKind::AddColumn => {
                if !self.require_text(&text, "column name") {
                    return Action::None;
                }
                match self.board.add_column(text.clone()) {
                    Ok(index) => {
                        self.remember_current_column_cursor();
                        self.selected_column = index;
                        self.selected_task = None;
                        self.remember_current_column_cursor();
                        self.restore_return_mode(input.return_to);
                        Action::Save(format!("Add column {text}"))
                    }
                    Err(error) => {
                        self.status = Some(error.into());
                        Action::None
                    }
                }
            }
            InputKind::RenameColumn => {
                if !self.require_text(&text, "column name") {
                    return Action::None;
                }
                self.current_column_mut().title = text.clone();
                self.restore_return_mode(input.return_to);
                Action::Save(format!("Rename column to {text}"))
            }
            InputKind::AddTask => {
                if !self.require_text(&text, "task title") {
                    return Action::None;
                }
                let index = self.board.add_task(self.selected_column, text.clone());
                self.selected_task = Some(index);
                self.remember_current_column_cursor();
                self.follow_column_cursor(self.selected_column);
                self.restore_return_mode(input.return_to);
                Action::Save(format!("Add task {text}"))
            }
            InputKind::TaskTitle => {
                if !self.require_text(&text, "task title") {
                    return Action::None;
                }
                self.current_task_mut().title = text;
                self.restore_return_mode(input.return_to);
                Action::Save("Edit task title".into())
            }
            InputKind::TaskDescription => {
                self.current_task_mut().description = text;
                self.restore_return_mode(input.return_to);
                Action::Save("Edit task description".into())
            }
            InputKind::AddChecklistItem => {
                if !self.require_text(&text, "checklist item") {
                    return Action::None;
                }
                self.current_task_mut()
                    .checklist
                    .push(ChecklistItem::new(text));
                self.restore_return_mode(input.return_to);
                Action::Save("Add checklist item".into())
            }
            InputKind::EditChecklistItem(index) => {
                if !self.require_text(&text, "checklist item") {
                    return Action::None;
                }
                self.current_task_mut().checklist[index].text = text;
                self.restore_return_mode(input.return_to);
                Action::Save("Edit checklist item".into())
            }
        }
    }

    fn edit_task_field(&mut self, cursor: usize) {
        let checklist_len = self.current_task().checklist.len();
        if cursor == 2 {
            self.open_tag_picker(ReturnTo::TaskDetails(cursor));
            return;
        }
        if cursor == 3 {
            self.open_date_picker(ReturnTo::TaskDetails(cursor));
            return;
        }
        let (kind, initial) = match cursor {
            0 => (InputKind::TaskTitle, self.current_task().title.clone()),
            1 => (
                InputKind::TaskDescription,
                self.current_task().description.clone(),
            ),
            value if value >= 4 && value < 4 + checklist_len => {
                let index = value - 4;
                (
                    InputKind::EditChecklistItem(index),
                    self.current_task().checklist[index].text.clone(),
                )
            }
            _ => return,
        };
        self.open_input(kind, initial, ReturnTo::TaskDetails(cursor));
    }

    fn begin_delete(&mut self) {
        let target = if let Some(task) = self.selected_task {
            DeleteTarget::Task {
                column: self.selected_column,
                task,
            }
        } else if self.selected_column == 0 {
            self.status =
                Some("the first column cannot be deleted because it has no prior column".into());
            return;
        } else {
            DeleteTarget::Column {
                index: self.selected_column,
            }
        };
        self.mode = Mode::ConfirmDelete(DeleteConfirmation {
            target,
            choice: DeleteChoice::Cancel,
        });
    }

    fn confirm_delete(&mut self, target: DeleteTarget) -> Action {
        match target {
            DeleteTarget::Column { index } => {
                if index == 0 || index >= self.board.columns.len() {
                    self.mode = Mode::Board;
                    self.status = Some("column no longer exists".into());
                    return Action::None;
                }
                let id = self.board.columns[index].id;
                let title = self.board.columns[index].title.clone();
                let moved = match self.board.delete_column(index) {
                    Ok(moved) => moved,
                    Err(error) => {
                        self.mode = Mode::Board;
                        self.status = Some(error.into());
                        return Action::None;
                    }
                };
                self.selected_column = index - 1;
                self.selected_task = None;
                self.remember_current_column_cursor();
                self.mode = Mode::Board;
                Action::Save(format!(
                    "Delete column {id} ({title}); move {moved} tasks to prior column"
                ))
            }
            DeleteTarget::Task { column, task } => {
                let Some(tasks) = self
                    .board
                    .columns
                    .get_mut(column)
                    .map(|column| &mut column.tasks)
                else {
                    self.mode = Mode::Board;
                    self.status = Some("task no longer exists".into());
                    return Action::None;
                };
                if task >= tasks.len() {
                    self.mode = Mode::Board;
                    self.status = Some("task no longer exists".into());
                    return Action::None;
                }
                let removed = tasks.remove(task);
                self.selected_column = column;
                self.selected_task = if tasks.is_empty() {
                    None
                } else {
                    Some(task.min(tasks.len() - 1))
                };
                self.remember_current_column_cursor();
                if self.selected_task.is_some() {
                    self.follow_column_cursor(column);
                }
                self.mode = Mode::Board;
                Action::Save(format!("Delete task {} ({})", removed.id, removed.title))
            }
        }
    }

    fn begin_move(&mut self) {
        let task = self.selected_task.unwrap();
        self.mode = Mode::Moving(MoveState {
            snapshot: self.board.clone(),
            origin_column: self.selected_column,
            origin_task: task,
        });
        self.status = Some("moving: arrows/hjkl move · enter/m confirms · esc cancels".into());
    }

    fn move_task_vertically(&mut self, delta: isize) {
        let current = self.selected_task.unwrap();
        let len = self.current_column().tasks.len();
        let target = offset_index(current, delta, len);
        if target != current {
            self.current_column_mut().tasks.swap(current, target);
            self.selected_task = Some(target);
            self.remember_current_column_cursor();
            self.follow_column_cursor(self.selected_column);
        }
    }

    fn move_task_horizontally(&mut self, delta: isize) {
        let target_column = offset_index(self.selected_column, delta, self.board.columns.len());
        if target_column == self.selected_column {
            return;
        }
        let task_index = self.selected_task.unwrap();
        let task = self.current_column_mut().tasks.remove(task_index);
        let target_index = task_index.min(self.board.columns[target_column].tasks.len());
        self.board.columns[target_column]
            .tasks
            .insert(target_index, task);
        self.selected_column = target_column;
        self.selected_task = Some(target_index);
        self.remember_current_column_cursor();
        self.follow_column_cursor(target_column);
    }

    fn select_up(&mut self) {
        self.selected_task = self.selected_task.and_then(|task| task.checked_sub(1));
        self.remember_current_column_cursor();
        self.follow_column_cursor(self.selected_column);
    }

    fn select_down(&mut self) {
        let len = self.current_column().tasks.len();
        self.selected_task = match self.selected_task {
            None if len > 0 => Some(0),
            Some(task) if task + 1 < len => Some(task + 1),
            current => current,
        };
        self.remember_current_column_cursor();
        self.follow_column_cursor(self.selected_column);
    }

    fn select_column(&mut self, delta: isize) {
        let target = offset_index(self.selected_column, delta, self.board.columns.len());
        self.jump_to_column(target);
    }

    fn jump_to_column(&mut self, target: usize) {
        if target >= self.board.columns.len() || target == self.selected_column {
            return;
        }
        self.remember_current_column_cursor();
        self.selected_column = target;
        let len = self.current_column().tasks.len();
        self.selected_task = self
            .column_cursor(target)
            .map(|task| task.min(len.saturating_sub(1)))
            .filter(|_| len > 0);
        self.remember_current_column_cursor();
        if self.selected_task.is_some() {
            self.follow_column_cursor(target);
        }
    }

    fn open_input(&mut self, kind: InputKind, text: impl Into<String>, return_to: ReturnTo) {
        let text = text.into();
        self.mode = Mode::Input(InputState {
            kind,
            editor: TextEditor::new(text),
            return_to,
        });
    }

    fn open_date_picker(&mut self, return_to: ReturnTo) {
        let original = self.current_task().due_date;
        self.mode = Mode::DatePicker(DatePickerState {
            selected: original.unwrap_or_else(|| Local::now().date_naive()),
            original,
            return_to,
        });
    }

    fn open_tag_picker(&mut self, return_to: ReturnTo) {
        self.mode = Mode::TagPicker(TagPickerState {
            row: if self.current_task().tags.is_empty() {
                TagPickerRow::Available
            } else {
                TagPickerRow::Current
            },
            index: 0,
            return_to,
        });
    }

    fn open_new_tag(&mut self, picker: TagPickerState) {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as usize)
            .unwrap_or(0)
            ^ self.board.next_tag_id as usize;
        self.mode = Mode::NewTag(NewTagState {
            name: TextEditor::new(""),
            color_index: seed % TAG_COLOR_PALETTE.len(),
            field: NewTagField::Name,
            picker,
        });
    }

    fn submit_new_tag(&mut self, mut state: NewTagState) -> Action {
        let name = state.name.text().trim().trim_start_matches('#').to_owned();
        if name.is_empty() {
            self.status = Some("tag name cannot be empty".into());
            self.mode = Mode::NewTag(state);
            return Action::None;
        }
        let color = TAG_COLOR_PALETTE[state.color_index];
        if let Err(error) = self.board.create_tag(&name, color) {
            self.status = Some(error);
            self.mode = Mode::NewTag(state);
            return Action::None;
        }
        self.current_task_mut().tags.push(name.clone());
        state.picker.row = TagPickerRow::Current;
        state.picker.index = self.current_task().tags.len() - 1;
        self.mode = Mode::TagPicker(state.picker);
        Action::Save(format!("Create tag {name}"))
    }

    pub fn available_tag_names(&self) -> Vec<String> {
        self.board
            .tags
            .iter()
            .filter(|tag| {
                !self
                    .current_task()
                    .tags
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&tag.name))
            })
            .map(|tag| tag.name.clone())
            .collect()
    }

    fn move_picker_days(&mut self, days: i64) {
        if let Mode::DatePicker(picker) = &mut self.mode
            && let Some(date) = picker.selected.checked_add_signed(Duration::days(days))
        {
            picker.selected = date;
        }
    }

    fn move_picker_months(&mut self, months: i32) {
        if let Mode::DatePicker(picker) = &mut self.mode
            && let Some(date) = shift_month(picker.selected, months)
        {
            picker.selected = date;
        }
    }

    fn restore_return_mode(&mut self, return_to: ReturnTo) {
        self.mode = match return_to {
            ReturnTo::Board => Mode::Board,
            ReturnTo::TaskDetails(cursor) => Mode::TaskDetails { cursor },
            ReturnTo::ColumnDetails(cursor) => Mode::ColumnDetails { cursor },
        };
    }

    fn close_floating_mode(&mut self) -> bool {
        match self.mode.clone() {
            Mode::Board | Mode::Moving(_) => return false,
            Mode::TaskDetails { .. } | Mode::ColumnDetails { .. } | Mode::ConfirmDelete(_) => {
                self.mode = Mode::Board
            }
            Mode::Input(input) => self.restore_return_mode(input.return_to),
            Mode::DatePicker(picker) => self.restore_return_mode(picker.return_to),
            Mode::TagPicker(picker) => self.restore_return_mode(picker.return_to),
            Mode::NewTag(state) => self.mode = Mode::TagPicker(state.picker),
            Mode::Help { return_to } => self.mode = *return_to,
        }
        true
    }

    fn require_text(&mut self, value: &str, field: &str) -> bool {
        if value.is_empty() {
            self.status = Some(format!("{field} cannot be empty"));
            false
        } else {
            true
        }
    }

    fn checklist_index_at(&self, cursor: usize) -> Option<usize> {
        let index = cursor.checked_sub(4)?;
        (index < self.current_task().checklist.len()).then_some(index)
    }

    pub fn task_detail_count(&self) -> usize {
        self.current_task().checklist.len() + 4
    }

    pub fn current_column(&self) -> &crate::model::Column {
        &self.board.columns[self.selected_column]
    }

    fn current_column_mut(&mut self) -> &mut crate::model::Column {
        &mut self.board.columns[self.selected_column]
    }

    pub fn current_task(&self) -> &crate::model::Task {
        &self.current_column().tasks[self.selected_task.unwrap()]
    }

    pub fn active_text_editor_content(&self) -> Option<String> {
        match &self.mode {
            Mode::Input(input) => Some(input.editor.text()),
            Mode::NewTag(state) if state.field == NewTagField::Name => Some(state.name.text()),
            _ => None,
        }
    }

    pub fn replace_active_text_editor_content(&mut self, text: &str) -> bool {
        let normalized = text.replace("\r\n", "\n");
        match &mut self.mode {
            Mode::Input(input) => {
                let text = if input.kind.allows_multiline() {
                    normalized
                } else {
                    normalized.split('\n').collect::<Vec<_>>().join(" ")
                };
                input.editor.set_text(&text);
                true
            }
            Mode::NewTag(state) if state.field == NewTagField::Name => {
                state
                    .name
                    .set_text(&normalized.split('\n').collect::<Vec<_>>().join(" "));
                true
            }
            _ => false,
        }
    }

    fn is_text_editor_active(&self) -> bool {
        matches!(&self.mode, Mode::Input(_))
            || matches!(
                &self.mode,
                Mode::NewTag(NewTagState {
                    field: NewTagField::Name,
                    ..
                })
            )
    }

    fn current_task_mut(&mut self) -> &mut crate::model::Task {
        let column = self.selected_column;
        let task = self.selected_task.unwrap();
        &mut self.board.columns[column].tasks[task]
    }
}

impl InputKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::AddColumn => "Add column",
            Self::RenameColumn => "Rename column",
            Self::AddTask => "Add task",
            Self::TaskTitle => "Edit title",
            Self::TaskDescription => "Edit description",
            Self::AddChecklistItem => "Add checklist item",
            Self::EditChecklistItem(_) => "Edit checklist item",
        }
    }

    fn allows_multiline(&self) -> bool {
        matches!(self, Self::TaskDescription)
    }
}

fn shift_month(date: NaiveDate, months: i32) -> Option<NaiveDate> {
    let month_index = date
        .year()
        .checked_mul(12)?
        .checked_add(date.month0() as i32)?
        .checked_add(months)?;
    let year = month_index.div_euclid(12);
    let month = month_index.rem_euclid(12) as u32 + 1;
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year.checked_add(1)?, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    let last_day = next_month.pred_opt()?.day();
    NaiveDate::from_ymd_opt(year, month, date.day().min(last_day))
}

fn offset_index(current: usize, delta: isize, len: usize) -> usize {
    if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current
            .saturating_add(delta as usize)
            .min(len.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEvent;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn cursor_can_select_header_and_tasks() {
        let mut board = Board::default();
        board.add_task(0, "one".into());
        board.add_task(0, "two".into());
        let mut app = App::new(board);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_task, Some(0));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_task, Some(1));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_task, None);
    }

    #[test]
    fn horizontal_navigation_restores_each_columns_last_cursor() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        for column in 0..2 {
            for index in 0..5 {
                board.add_task(column, format!("Task {column}-{index}"));
            }
        }
        let mut app = App::new(board);

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_task, Some(1));

        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_task, None);
        for _ in 0..4 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.selected_task, Some(3));

        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.selected_column, 0);
        assert_eq!(app.selected_task, Some(1));
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_task, Some(3));
    }

    #[test]
    fn cancelled_move_restores_the_board() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        board.add_task(0, "one".into());
        let original = board.clone();
        let mut app = App::new(board);
        app.selected_task = Some(0);

        app.handle_key(key(KeyCode::Char('m')));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.board, original);
        assert_eq!(app.selected_column, 0);
        assert_eq!(app.selected_task, Some(0));
    }

    #[test]
    fn help_restores_the_mode_it_was_opened_from() {
        let mut app = App::new(Board::default());
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('N')));
        app.handle_key(key(KeyCode::Char('?')));
        assert!(matches!(&app.mode, Mode::Input(_)));
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Help { .. }));

        app.handle_key(key(KeyCode::Esc));

        let Mode::Input(input) = &app.mode else {
            panic!("help did not restore the input dialog");
        };
        assert_eq!(input.editor.text(), "N?");
        // Legacy terminal encoding sends Ctrl-/ as the same byte as Ctrl-_, which
        // crossterm exposes as Ctrl-7. Enhanced keyboard protocols retain '/'.
        app.handle_key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Help { .. }));
    }

    #[test]
    fn input_editor_moves_inserts_deletes_and_requests_external_editing() {
        let mut app = App::new(Board::default());
        app.handle_key(key(KeyCode::Char('a')));
        for character in "abcd".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Left));
        app.handle_key(key(KeyCode::Left));
        app.handle_key(key(KeyCode::Char('X')));
        app.handle_key(key(KeyCode::Backspace));
        app.handle_key(key(KeyCode::Home));
        app.handle_key(key(KeyCode::Delete));
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Char('?')));
        app.handle_key(key(KeyCode::Char('q')));

        let Mode::Input(input) = &app.mode else {
            panic!("text editing should keep the input dialog open");
        };
        assert_eq!(input.editor.text(), "bcd?q");
        assert_eq!(input.editor.cursor(), (0, 5));
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            Action::EditTextExternally
        );
        assert!(app.replace_active_text_editor_content("external\nedit"));
        let Mode::Input(input) = &app.mode else {
            unreachable!()
        };
        assert_eq!(input.editor.text(), "external edit");
        assert_eq!(input.editor.cursor(), (0, 13));
    }

    #[test]
    fn external_task_description_edits_keep_multiple_lines() {
        let mut board = Board::default();
        board.add_task(0, "described".into());
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 1 };
        app.handle_key(key(KeyCode::Enter));

        assert!(app.replace_active_text_editor_content("first line\nsecond line"));
        let Mode::Input(input) = &app.mode else {
            panic!("description input should remain open");
        };
        assert_eq!(input.editor.text(), "first line\nsecond line");
        assert_eq!(input.editor.cursor(), (1, 11));
    }

    #[test]
    fn q_types_in_text_fields_but_closes_other_floats_and_ctrl_c_always_quits() {
        let mut app = App::new(Board::default());
        app.mode = Mode::ColumnDetails { cursor: 0 };
        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::None);
        assert!(matches!(&app.mode, Mode::Board));

        app.mode = Mode::Input(InputState {
            kind: InputKind::AddTask,
            editor: TextEditor::new(""),
            return_to: ReturnTo::Board,
        });
        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::None);
        let Mode::Input(input) = &app.mode else {
            panic!("q should keep the input dialog open");
        };
        assert_eq!(input.editor.text(), "q");
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(&app.mode, Mode::Board));

        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit);

        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn q_closes_only_the_help_window_when_help_is_open() {
        let mut app = App::new(Board::default());
        app.mode = Mode::ColumnDetails { cursor: 0 };
        app.handle_key(key(KeyCode::Char('?')));

        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::None);
        assert!(matches!(&app.mode, Mode::ColumnDetails { cursor: 0 }));
    }

    #[test]
    fn delete_task_requires_confirmation_and_preserves_a_nearby_selection() {
        let mut board = Board::default();
        board.add_task(0, "first".into());
        board.add_task(0, "second".into());
        let mut app = App::new(board);
        app.selected_task = Some(0);

        app.handle_key(key(KeyCode::Char('D')));
        assert!(matches!(
            &app.mode,
            Mode::ConfirmDelete(DeleteConfirmation {
                target: DeleteTarget::Task { column: 0, task: 0 },
                choice: DeleteChoice::Cancel,
            })
        ));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.current_column().tasks.len(), 2);

        app.handle_key(key(KeyCode::Char('D')));
        app.handle_key(key(KeyCode::Right));
        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Save(message) if message.starts_with("Delete task ")
        ));
        assert_eq!(app.current_column().tasks.len(), 1);
        assert_eq!(app.current_task().title, "second");
        assert_eq!(app.selected_task, Some(0));
        assert!(matches!(&app.mode, Mode::Board));
    }

    #[test]
    fn delete_column_moves_tasks_to_the_prior_column_after_confirmation() {
        let mut board = Board::default();
        board.add_column("DOING".into()).unwrap();
        board.add_task(0, "existing".into());
        board.add_task(1, "moved".into());
        let mut app = App::new(board);
        app.selected_column = 1;

        app.handle_key(key(KeyCode::Char('D')));
        assert!(matches!(
            &app.mode,
            Mode::ConfirmDelete(DeleteConfirmation {
                target: DeleteTarget::Column { index: 1 },
                choice: DeleteChoice::Cancel,
            })
        ));
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('y'))),
            Action::Save(message) if message.starts_with("Delete column ")
        ));

        assert_eq!(app.board.columns.len(), 1);
        assert_eq!(app.board.columns[0].tasks.len(), 2);
        assert_eq!(app.board.columns[0].tasks[1].title, "moved");
        assert_eq!(app.selected_column, 0);
        assert_eq!(app.selected_task, None);
        assert!(matches!(&app.mode, Mode::Board));

        app.handle_key(key(KeyCode::Char('D')));
        assert!(matches!(&app.mode, Mode::Board));
        assert_eq!(
            app.status.as_deref(),
            Some("the first column cannot be deleted because it has no prior column")
        );
    }

    #[test]
    fn date_picker_clamps_months_cancels_and_commits() {
        let mut board = Board::default();
        board.add_task(0, "dated".into());
        board.columns[0].tasks[0].due_date = NaiveDate::from_ymd_opt(2026, 1, 31);
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 3 };

        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::PageDown));
        let Mode::DatePicker(picker) = &app.mode else {
            panic!("due date did not open the date picker");
        };
        assert_eq!(
            picker.selected,
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(
            app.current_task().due_date,
            NaiveDate::from_ymd_opt(2026, 1, 31)
        );

        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Save("Edit task due date".into())
        );
        assert_eq!(
            app.current_task().due_date,
            NaiveDate::from_ymd_opt(2026, 2, 28)
        );

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('d'))),
            Action::Save("Clear task due date".into())
        );
        assert_eq!(app.current_task().due_date, None);
    }

    #[test]
    fn tag_picker_adds_existing_and_creates_colored_tags() {
        let mut board = Board::default();
        board.add_task(0, "tagged".into());
        board.create_tag("existing", "#E06C75").unwrap();
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 2 };

        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::TagPicker(_)));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Save("Add tag existing".into())
        );
        assert_eq!(app.current_task().tags, ["existing"]);

        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::NewTag(_)));
        for character in "project".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        let Mode::NewTag(state) = &app.mode else {
            panic!("new tag name did not advance to the color picker");
        };
        let color = TAG_COLOR_PALETTE[state.color_index];
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Save("Create tag project".into())
        );
        assert_eq!(app.current_task().tags, ["existing", "project"]);
        assert_eq!(app.board.tag_by_name("project").unwrap().color, color);
    }

    #[test]
    fn checklist_rows_follow_metadata_and_toggles_record_completion_time() {
        let mut board = Board::default();
        board.add_task(0, "checklist".into());
        board.columns[0].tasks[0]
            .checklist
            .push(ChecklistItem::new("Run tests".into()));
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 4 };

        assert_eq!(
            app.handle_key(key(KeyCode::Char(' '))),
            Action::Save("Toggle checklist item".into())
        );
        assert!(app.current_task().checklist[0].completed);
        assert!(app.current_task().checklist[0].completed_at.is_some());
    }

    #[test]
    fn mouse_style_checklist_actions_select_toggle_and_scroll_details() {
        let mut board = Board::default();
        board.add_task(0, "checklist".into());
        board.columns[0].tasks[0]
            .checklist
            .push(ChecklistItem::new("Click me".into()));
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.open_selected_task_details();

        assert_eq!(
            app.click_checklist_item(0),
            Action::Save("Toggle checklist item".into())
        );
        assert!(app.current_task().checklist[0].completed);
        assert!(matches!(app.mode, Mode::TaskDetails { cursor: 4 }));
        app.scroll_task_details(6);
        assert_eq!(app.task_detail_scroll, 6);
        assert!(!app.task_detail_follow_cursor);
        app.close_task_details();
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn external_reload_follows_a_selected_task_by_stable_id() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        board.add_task(0, "selected".into());
        let mut app = App::new(board.clone());
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 3 };

        let task = board.columns[0].tasks.remove(0);
        board.columns[1].tasks.push(task);
        app.replace_board_from_external_change(board);

        assert_eq!(app.selected_column, 1);
        assert_eq!(app.selected_task, Some(0));
        assert_eq!(app.current_task().title, "selected");
        assert!(matches!(app.mode, Mode::TaskDetails { cursor: 3 }));
        assert_eq!(
            app.status.as_deref(),
            Some("reloaded external board changes")
        );
    }
}
