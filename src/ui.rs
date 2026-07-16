use anyhow::{Result, bail};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};

use crate::{
    app::{
        App, DeleteChoice, DeleteConfirmation, DeleteTarget, Mode, NewTagField, TagPickerRow,
        TextEditor,
    },
    config::ThemeConfig,
    history::{TaskHistoryEvent, TaskHistoryKind, checklist_status_item},
    model::{ChecklistItem, TAG_COLOR_PALETTE, TagDefinition, Task},
};

const MIN_COLUMN_WIDTH: u16 = 26;
const TASK_CURSOR_WIDTH: u16 = 2;
const TASK_CARD_GAP: u16 = 1;
const TASK_CARD_RIGHT_PADDING: u16 = 1;
const TUI_BACKGROUND: Color = Color::Rgb(0, 0, 0);
const TUI_ACCENT: Color = Color::Rgb(255, 135, 0);

pub struct Theme {
    background: Color,
    accent: Color,
    selected_background: Color,
    border: Color,
    text: Color,
    muted: Color,
    danger: Color,
    success: Color,
    change: Color,
    mouse_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HitTarget {
    Column(usize),
    Task { column: usize, task: usize },
    TaskDetailsClose,
    ChecklistItem(usize),
}

struct TaskDetailField {
    lines: Vec<Line<'static>>,
    checklist_index: Option<usize>,
    clickable_start: usize,
}

struct TaskDetailDocument {
    lines: Vec<Line<'static>>,
    field_ranges: Vec<std::ops::Range<usize>>,
    checklist_ranges: Vec<(usize, std::ops::Range<usize>)>,
}

struct ColumnCardLayout {
    start: usize,
    cards: Vec<(usize, Rect)>,
    hidden_above: usize,
    hidden_below: usize,
}

impl Theme {
    #[cfg(test)]
    pub fn from_config(config: &ThemeConfig) -> Result<Self> {
        Self::from_config_with_mouse(config, true)
    }

    pub fn from_config_with_mouse(config: &ThemeConfig, mouse_enabled: bool) -> Result<Self> {
        Ok(Self {
            background: TUI_BACKGROUND,
            accent: TUI_ACCENT,
            selected_background: parse_color(&config.selected_background)?,
            border: parse_color(&config.border)?,
            text: parse_color(&config.text)?,
            muted: parse_color(&config.muted)?,
            danger: parse_color(&config.danger)?,
            success: parse_color(&config.success)?,
            change: parse_color(&config.change)?,
            mouse_enabled,
        })
    }
}

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        frame.area(),
    );
    let [board_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    draw_board(frame, board_area, app, theme);
    draw_footer(frame, footer_area, app, theme);

    match &app.mode {
        Mode::TaskDetails { cursor } => draw_task_details(frame, app, *cursor, theme),
        Mode::ColumnDetails { cursor } => draw_column_details(frame, app, *cursor, theme),
        Mode::Input(input) => draw_input(frame, input, app, theme),
        Mode::DatePicker(picker) => draw_date_picker(frame, picker, theme),
        Mode::TagPicker(picker) => draw_tag_picker(frame, app, picker, theme),
        Mode::NewTag(state) => draw_new_tag(frame, app, state, theme),
        Mode::ConfirmDelete(state) => draw_delete_confirmation(frame, app, state, theme),
        Mode::Help { return_to } if mode_has_text_editor(return_to) => {
            draw_text_input_help(frame, theme)
        }
        Mode::Help { .. } => draw_help(frame, theme),
        Mode::Board | Mode::Moving(_) => {}
    }
}

fn mode_has_text_editor(mode: &Mode) -> bool {
    matches!(mode, Mode::Input(_))
        || matches!(
            mode,
            Mode::NewTag(crate::app::NewTagState {
                field: NewTagField::Name,
                ..
            })
        )
}

pub fn hit_test(area: Rect, app: &App, x: u16, y: u16, theme: &Theme) -> Option<HitTarget> {
    if !theme.mouse_enabled {
        return None;
    }
    if let Mode::TaskDetails { cursor } = app.mode {
        let (modal, document) = task_detail_layout(area, app, cursor, theme);
        if contains(task_details_close_area(modal), x, y) {
            return Some(HitTarget::TaskDetailsClose);
        }
        let content = modal_content_area(modal);
        if !contains(content, x, y) {
            return None;
        }
        let content_height = usize::from(content.height).max(1);
        let scroll = task_detail_scroll(app, &document, cursor, content_height);
        let mut visible_line = usize::from(y.saturating_sub(content.y));
        if task_detail_has_scroll_hints(document.lines.len(), content_height) {
            if visible_line == 0 {
                return None;
            }
            visible_line -= 1;
        }
        let document_line = scroll + visible_line;
        for (index, range) in &document.checklist_ranges {
            if range.contains(&document_line) {
                return Some(HitTarget::ChecklistItem(*index));
            }
        }
        return None;
    }
    let [board_area, _] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    for (column_index, lane) in visible_lanes(board_area, app) {
        if !contains(lane, x, y) {
            continue;
        }
        let (header, body) = lane_regions(lane);
        if contains(header, x, y) {
            return Some(HitTarget::Column(column_index));
        }
        let inner = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .inner(body);
        for (task_index, card) in visible_cards(inner, app, column_index) {
            if contains(card, x, y) {
                return Some(HitTarget::Task {
                    column: column_index,
                    task: task_index,
                });
            }
        }
        return Some(HitTarget::Column(column_index));
    }
    None
}

pub fn scroll_task_details(area: Rect, app: &mut App, lines: isize, theme: &Theme) {
    let Mode::TaskDetails { cursor } = app.mode else {
        return;
    };
    let (modal, document) = task_detail_layout(area, app, cursor, theme);
    let content_height = usize::from(modal_content_area(modal).height).max(1);
    let document_height = task_detail_visible_rows(document.lines.len(), content_height);
    let max_scroll = document.lines.len().saturating_sub(document_height);
    app.task_detail_scroll = task_detail_scroll(app, &document, cursor, content_height);
    app.scroll_task_details(lines);
    app.task_detail_scroll = app.task_detail_scroll.min(max_scroll);
}

pub fn scroll_task_details_half_page(area: Rect, app: &mut App, down: bool, theme: &Theme) {
    let Mode::TaskDetails { cursor } = app.mode else {
        return;
    };
    let (modal, document) = task_detail_layout(area, app, cursor, theme);
    let content_height = usize::from(modal_content_area(modal).height).max(1);
    let half_page = task_detail_visible_rows(document.lines.len(), content_height) / 2;
    let lines = isize::try_from(half_page.max(1)).unwrap_or(isize::MAX);
    scroll_task_details(area, app, if down { lines } else { -lines }, theme);
}

pub fn prepare_board_scrolls(area: Rect, app: &mut App) {
    let [board_area, _] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let lanes = visible_lanes(board_area, app);
    for (column_index, lane) in lanes {
        let body = lane_regions(lane).1;
        let inner = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .inner(body);
        let start = column_card_layout(inner, app, column_index).start;
        app.set_column_scroll(column_index, start);
    }
}

pub fn scroll_at(area: Rect, app: &mut App, x: u16, y: u16, lines: isize, theme: &Theme) {
    if lines == 0 {
        return;
    }
    if let Mode::TaskDetails { cursor } = app.mode {
        let (modal, _) = task_detail_layout(area, app, cursor, theme);
        if contains(modal, x, y) {
            scroll_task_details(area, app, lines, theme);
        }
        return;
    }
    if !matches!(app.mode, Mode::Board | Mode::Moving(_)) {
        return;
    }

    let [board_area, _] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    if let Some((column_index, _)) = visible_lanes(board_area, app)
        .into_iter()
        .find(|(_, lane)| contains(*lane, x, y))
    {
        app.scroll_column(column_index, lines.signum());
    }
}

fn draw_board(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for (column_index, lane) in visible_lanes(area, app) {
        let column = &app.board.columns[column_index];
        let is_column = column_index == app.selected_column;
        let header_selected = is_column && app.selected_task.is_none();
        let header_style = if header_selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        };
        let moving = is_column && matches!(app.mode, Mode::Moving(_));
        let title = if moving {
            format!("{} {} · MOVE", column_index + 1, column.title)
        } else {
            format!("{} {}", column_index + 1, column.title)
        };
        let (header, body) = lane_regions(lane);
        let border_color = if header_selected {
            theme.accent
        } else {
            theme.border
        };
        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let header_inner = header_block.inner(header);
        frame.render_widget(header_block, header);
        let cursor_width = TASK_CURSOR_WIDTH.min(header_inner.width);
        let right_padding = cursor_width.min(header_inner.width.saturating_sub(cursor_width));
        let [cursor_area, title_area, _] = Layout::horizontal([
            Constraint::Length(cursor_width),
            Constraint::Min(0),
            Constraint::Length(right_padding),
        ])
        .areas(header_inner);
        if header_selected {
            frame.render_widget(
                Paragraph::new("▊").style(Style::default().fg(theme.accent)),
                cursor_area,
            );
        }
        frame.render_widget(
            Paragraph::new(title)
                .alignment(Alignment::Center)
                .style(header_style),
            title_area,
        );
        let body_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(border_color));
        let inner = body_block.inner(body);
        frame.render_widget(body_block, body);
        if header.height >= 3 && header.width >= 2 && body.height > 0 {
            let junction_style = Style::default().fg(border_color);
            let separator_width = header.width.saturating_sub(2);
            frame.render_widget(
                Paragraph::new("─".repeat(usize::from(separator_width))).style(junction_style),
                Rect::new(
                    header.x.saturating_add(1),
                    header.bottom() - 1,
                    separator_width,
                    1,
                ),
            );
            frame.render_widget(
                Paragraph::new("├").style(junction_style),
                Rect::new(header.x, header.bottom() - 1, 1, 1),
            );
            frame.render_widget(
                Paragraph::new("┤").style(junction_style),
                Rect::new(header.right() - 1, header.bottom() - 1, 1, 1),
            );
        }
        draw_cards(frame, inner, app, column_index, theme);
    }
}

fn draw_cards(frame: &mut Frame, area: Rect, app: &App, column_index: usize, theme: &Theme) {
    let tasks = &app.board.columns[column_index].tasks;
    if tasks.is_empty() {
        frame.render_widget(
            Paragraph::new(" No tasks · a to add").style(Style::default().fg(theme.muted)),
            area,
        );
        return;
    }
    let selected = (column_index == app.selected_column)
        .then_some(app.selected_task)
        .flatten();
    let layout = column_card_layout(area, app, column_index);
    if layout.hidden_above > 0 {
        frame.render_widget(
            Paragraph::new(format!("↑ ({} more)", layout.hidden_above))
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }
    if layout.hidden_below > 0 {
        frame.render_widget(
            Paragraph::new(format!("↓ ({} more)", layout.hidden_below))
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        );
    }
    for (task_index, rect) in layout.cards {
        let task = &tasks[task_index];
        let selected = selected == Some(task_index);
        let moving = selected && matches!(app.mode, Mode::Moving(_));
        if moving {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent));
            let inner = block.inner(rect);
            let content_area = Rect::new(
                inner.x,
                inner.y,
                inner.width.saturating_sub(TASK_CARD_RIGHT_PADDING),
                inner.height,
            );
            frame.render_widget(block, rect);
            frame.render_widget(
                Paragraph::new(task_card_lines(app, task, content_area.width.max(1), theme)),
                content_area,
            );
            continue;
        }

        let cursor_width = TASK_CURSOR_WIDTH.min(rect.width);
        let right_padding = TASK_CARD_RIGHT_PADDING.min(rect.width.saturating_sub(cursor_width));
        let [cursor_area, content_area, _] = Layout::horizontal([
            Constraint::Length(cursor_width),
            Constraint::Min(0),
            Constraint::Length(right_padding),
        ])
        .areas(rect);
        if selected {
            let cursor = (0..cursor_area.height)
                .map(|_| Line::styled("▊", Style::default().fg(theme.accent)))
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(cursor), cursor_area);
        }
        frame.render_widget(
            Paragraph::new(task_card_lines(app, task, content_area.width.max(1), theme)),
            content_area,
        );
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let help = match app.mode {
        Mode::Board => {
            "BOARD mode · a add task · C add column · r rename · D delete · m MOVE · ? help · q quit"
        }
        Mode::Moving(_) => {
            "MOVE mode · arrows/hjkl reposition · enter/m confirm · esc cancel · q quit"
        }
        Mode::TaskDetails { .. } => {
            "DETAILS mode · arrows/hjkl select · enter activate · e edit · a add item · d delete item · esc/q close"
        }
        Mode::ColumnDetails { .. } => {
            "DETAILS mode · arrows/hjkl select · enter/e/r rename · esc/q close"
        }
        Mode::Input(_) => "INPUT mode · Ctrl-G $EDITOR · Ctrl-/ keymap",
        Mode::DatePicker(_) => {
            "DATE PICKER mode · arrows/hjkl select · PgUp/PgDn month · enter confirm · d clear · esc/q cancel"
        }
        Mode::TagPicker(_) => {
            "TAG PICKER mode · arrows/hjkl select · enter add/remove/create · esc/q close"
        }
        Mode::NewTag(_) => {
            "NEW TAG mode · Tab/arrows choose field · h/l choose color · Ctrl-G $EDITOR · Ctrl-/ keymap"
        }
        Mode::ConfirmDelete(_) => {
            "CONFIRM mode · left/right or h/l select · enter activate · y delete · esc/n/q cancel"
        }
        Mode::Help { .. } => "HELP mode · esc/q returns to where you were · Ctrl-C quits",
    };
    let floating = !matches!(app.mode, Mode::Board | Mode::Moving(_));
    if floating {
        let hidden = if let Some(status) = &app.status {
            format!(" {status} · {help}")
        } else {
            help.to_owned()
        };
        frame.render_widget(
            Paragraph::new(hidden)
                .style(Style::default().fg(theme.background).bg(theme.background)),
            area,
        );
        return;
    }

    let mut spans = Vec::new();
    if let Some(status) = &app.status {
        let color = if status.starts_with("error:") {
            theme.danger
        } else {
            theme.accent
        };
        spans.push(Span::styled(
            format!(" {status} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("· "));
    }
    spans.push(Span::styled(help, Style::default().fg(theme.muted)));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.background)),
        area,
    );
}

fn draw_task_details(frame: &mut Frame, app: &App, cursor: usize, theme: &Theme) {
    let (area, document) = task_detail_layout(frame.area(), app, cursor, theme);
    let content_height = usize::from(modal_content_area(area).height).max(1);
    let lines = task_detail_window_lines(app, &document, cursor, content_height, theme);
    let more_below = task_detail_more_below(app, &document, cursor, content_height);
    render_task_detail_modal(
        frame,
        area,
        lines,
        if theme.mouse_enabled {
            "hjkl · ^u/^d/wheel scroll · Enter/e edit · a/d items · Esc/q close"
        } else {
            "hjkl · ^u/^d scroll · Enter/e edit · a/d items · Esc/q close"
        },
        more_below,
        theme,
    );
    if theme.mouse_enabled {
        frame.render_widget(
            Paragraph::new("[×]").style(
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
            task_details_close_area(area),
        );
    }
}

fn task_detail_fields(
    app: &App,
    task: &Task,
    cursor: usize,
    content_width: usize,
    theme: &Theme,
) -> Vec<TaskDetailField> {
    let fields = vec![
        ("Title", task.title.clone()),
        (
            "Description",
            if task.description.is_empty() {
                "—".into()
            } else {
                task.description.clone()
            },
        ),
        (
            "Tags",
            if task.tags.is_empty() {
                "—".into()
            } else {
                task.tags
                    .iter()
                    .map(|tag| format!("#{tag}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
        ),
        (
            "Due",
            task.due_date
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "—".into()),
        ),
    ];
    let label_width = fields
        .iter()
        .map(|(label, _)| Span::raw(*label).width())
        .max()
        .unwrap_or(0);

    let mut field_lines = fields
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let selected = index == cursor;
            let lines = if index == 2 && !task.tags.is_empty() {
                task_detail_tag_lines(
                    label,
                    label_width,
                    &task.tags,
                    selected,
                    content_width,
                    app,
                    theme,
                )
            } else {
                task_detail_text_lines(label, label_width, &value, selected, content_width, theme)
            };
            TaskDetailField {
                lines,
                checklist_index: None,
                clickable_start: 0,
            }
        })
        .collect::<Vec<_>>();
    if task.checklist.is_empty() {
        field_lines[3].lines.extend(checklist_section_header(theme));
        field_lines[3].lines.push(Line::styled(
            "    No checklist items — press a to add one",
            Style::default().fg(theme.muted),
        ));
    } else {
        for (index, item) in task.checklist.iter().enumerate() {
            let (added_at, completed_at) = checklist_item_times(app, task, index, item);
            let mut item_lines = checklist_item_lines(
                item,
                added_at,
                completed_at,
                cursor == index + 4,
                content_width,
                theme,
            );
            if index == 0 {
                let mut section = checklist_section_header(theme);
                section.append(&mut item_lines);
                item_lines = section;
            }
            field_lines.push(TaskDetailField {
                lines: item_lines,
                checklist_index: Some(index),
                clickable_start: usize::from(index == 0) * 2,
            });
        }
    }
    field_lines
}

fn task_detail_document(
    app: &App,
    task: &Task,
    cursor: usize,
    content_width: usize,
    theme: &Theme,
) -> TaskDetailDocument {
    let fields = task_detail_fields(app, task, cursor, content_width, theme);
    let mut lines = Vec::new();
    let mut field_ranges = Vec::with_capacity(fields.len());
    let mut checklist_ranges = Vec::new();
    for field in fields {
        let start = lines.len();
        lines.extend(field.lines);
        let end = lines.len();
        field_ranges.push(start..end);
        if let Some(index) = field.checklist_index {
            checklist_ranges.push((index, start + field.clickable_start..end));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  History",
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ));
    for group in task_history_line_groups(app, task, content_width, theme) {
        lines.extend(group);
    }
    let earlier = app.task_history_earlier.get(&task.id).copied().unwrap_or(0);
    if earlier > 0 {
        lines.push(Line::styled(
            format!(
                "  … {earlier} earlier event{} not loaded",
                if earlier == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme.muted),
        ));
    }
    TaskDetailDocument {
        lines,
        field_ranges,
        checklist_ranges,
    }
}

fn task_detail_layout(
    viewport: Rect,
    app: &App,
    cursor: usize,
    theme: &Theme,
) -> (Rect, TaskDetailDocument) {
    let width = viewport.width.saturating_mul(92).saturating_div(100).max(1);
    let content_width = usize::from(width.saturating_sub(2)).max(1);
    let document = task_detail_document(app, app.current_task(), cursor, content_width, theme);
    let max_height = viewport
        .height
        .saturating_mul(80)
        .saturating_div(100)
        .max(3)
        .min(viewport.height.max(1));
    let min_height = 12.min(max_height);
    let desired_height = usize_to_u16(document.lines.len().saturating_add(4));
    let height = desired_height.clamp(min_height, max_height);
    (centered(viewport, 92, height), document)
}

fn task_detail_scroll(
    app: &App,
    document: &TaskDetailDocument,
    cursor: usize,
    content_height: usize,
) -> usize {
    let document_height = task_detail_visible_rows(document.lines.len(), content_height);
    let max_scroll = document.lines.len().saturating_sub(document_height);
    let mut scroll = app.task_detail_scroll.min(max_scroll);
    if app.task_detail_follow_cursor
        && let Some(range) = document.field_ranges.get(cursor)
    {
        if range.start < scroll {
            scroll = range.start;
        } else if range.end > scroll.saturating_add(document_height) {
            scroll = range.end.saturating_sub(document_height);
        }
    }
    scroll.min(max_scroll)
}

fn task_detail_has_scroll_hints(document_height: usize, content_height: usize) -> bool {
    document_height > content_height && content_height >= 2
}

fn task_detail_visible_rows(document_height: usize, content_height: usize) -> usize {
    if task_detail_has_scroll_hints(document_height, content_height) {
        content_height - 1
    } else {
        content_height
    }
}

fn task_detail_window_lines(
    app: &App,
    document: &TaskDetailDocument,
    cursor: usize,
    content_height: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let scroll = task_detail_scroll(app, document, cursor, content_height);
    let visible_rows = task_detail_visible_rows(document.lines.len(), content_height);
    let end = scroll
        .saturating_add(visible_rows)
        .min(document.lines.len());
    if !task_detail_has_scroll_hints(document.lines.len(), content_height) {
        return document.lines[scroll..end].to_vec();
    }

    let indicator = |text: &'static str| -> Line<'static> {
        Line::styled(text, Style::default().fg(theme.muted)).alignment(Alignment::Center)
    };
    let mut lines = Vec::with_capacity(content_height);
    lines.push(if scroll > 0 {
        indicator("↑ (more)")
    } else {
        Line::raw("")
    });
    lines.extend(document.lines[scroll..end].iter().cloned());
    lines
}

fn task_detail_more_below(
    app: &App,
    document: &TaskDetailDocument,
    cursor: usize,
    content_height: usize,
) -> bool {
    let scroll = task_detail_scroll(app, document, cursor, content_height);
    let visible_rows = task_detail_visible_rows(document.lines.len(), content_height);
    scroll.saturating_add(visible_rows) < document.lines.len()
}

fn task_history_line_groups(
    app: &App,
    task: &Task,
    width: usize,
    theme: &Theme,
) -> Vec<Vec<Line<'static>>> {
    let fallback = [TaskHistoryEvent {
        at: task.created_at,
        kind: TaskHistoryKind::Created,
    }];
    let events = app
        .task_history
        .get(&task.id)
        .map(Vec::as_slice)
        .unwrap_or(&fallback);
    let now = Utc::now();
    let timestamp_width = events
        .iter()
        .map(|event| Span::raw(relative_time(event.at, now)).width())
        .max()
        .unwrap_or(0);
    let timestamp_prefix_width = 2 + timestamp_width + 2;
    let available = width.saturating_sub(timestamp_prefix_width);
    let column_gap = 2.min(available.saturating_sub(1));
    let usable = available.saturating_sub(column_gap);
    let minimum_content_width = usable.saturating_sub(1).clamp(1, 16);
    let desired_type_width = events
        .iter()
        .map(|event| Span::raw(history_event_type(event)).width())
        .max()
        .unwrap_or(1);
    let type_width = desired_type_width
        .min(usable.saturating_sub(minimum_content_width).max(1))
        .max(1);
    let content_width = usable.saturating_sub(type_width).max(1);

    events
        .iter()
        .rev()
        .map(|event| {
            let event_type = history_text_lines(
                &history_event_type(event),
                type_width,
                history_event_type_style(event, theme),
            );
            let content = history_event_content_lines(app, event, content_width, theme);
            let timestamp = relative_time(event.at, now);
            let row_count = event_type.len().max(content.len());
            let mut event_type = event_type.into_iter();
            let mut content = content.into_iter();
            (0..row_count)
                .map(|index| {
                    let mut spans = vec![Span::styled(
                        if index == 0 {
                            format!("  {timestamp:<timestamp_width$}  ")
                        } else {
                            " ".repeat(timestamp_prefix_width)
                        },
                        Style::default().fg(theme.muted),
                    )];
                    if let Some(line) = event_type.next() {
                        let line_width = line.width();
                        spans.extend(line.spans);
                        spans.push(Span::raw(" ".repeat(type_width.saturating_sub(line_width))));
                    } else {
                        spans.push(Span::raw(" ".repeat(type_width)));
                    }
                    spans.push(Span::raw(" ".repeat(column_gap)));
                    if let Some(line) = content.next() {
                        spans.extend(line.spans);
                    }
                    Line::from(spans)
                })
                .collect()
        })
        .collect()
}

fn history_event_type(event: &TaskHistoryEvent) -> String {
    match &event.kind {
        TaskHistoryKind::Created => "created".into(),
        TaskHistoryKind::Moved {
            from_column,
            to_column,
            ..
        } if from_column == to_column => "reordered".into(),
        TaskHistoryKind::Moved { .. } => "moved".into(),
        TaskHistoryKind::Changed { field, to, .. }
            if checklist_status_item(field).is_some() && to == "complete" =>
        {
            "checked".into()
        }
        TaskHistoryKind::Changed { field, to, .. }
            if checklist_status_item(field).is_some() && to == "incomplete" =>
        {
            "unchecked".into()
        }
        TaskHistoryKind::Changed { field, .. } => format!("changed {field}"),
        TaskHistoryKind::Added { field, .. } if is_checklist_item_field(field) => "added".into(),
        TaskHistoryKind::Removed { field, .. } if is_checklist_item_field(field) => {
            "removed".into()
        }
        TaskHistoryKind::Added { field, .. } => format!("added {field}"),
        TaskHistoryKind::Removed { field, .. } => format!("removed {field}"),
        TaskHistoryKind::TagAdded(_) => "added tag".into(),
        TaskHistoryKind::TagRemoved(_) => "removed tag".into(),
    }
}

fn history_event_type_style(event: &TaskHistoryEvent, theme: &Theme) -> Style {
    let color = match &event.kind {
        TaskHistoryKind::Created => theme.text,
        TaskHistoryKind::Added { .. } | TaskHistoryKind::TagAdded(_) => theme.success,
        TaskHistoryKind::Removed { .. } | TaskHistoryKind::TagRemoved(_) => theme.danger,
        TaskHistoryKind::Changed { .. } => theme.change,
        TaskHistoryKind::Moved { .. } => theme.muted,
    };
    Style::default().fg(color)
}

fn is_checklist_item_field(field: &str) -> bool {
    field
        .strip_prefix("checklist item ")
        .is_some_and(|index| index.parse::<usize>().is_ok())
}

fn history_event_content_lines(
    app: &App,
    event: &TaskHistoryEvent,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.muted);
    match &event.kind {
        TaskHistoryKind::Created => history_text_lines("task", width, muted),
        TaskHistoryKind::Moved {
            from_column,
            to_column,
            ..
        } if from_column == to_column => {
            history_text_lines(&format!("within {from_column}"), width, muted)
        }
        TaskHistoryKind::Moved {
            from_column,
            to_column,
            ..
        } => history_text_lines(&format!("{from_column} → {to_column}"), width, muted),
        TaskHistoryKind::Changed { field, from, to } => {
            if let Some(item) = checklist_status_item(field)
                && matches!(to.as_str(), "complete" | "incomplete")
            {
                return history_text_lines(&item, width, muted);
            }
            let mut lines = history_text_lines("from:", width, muted);
            lines.extend(history_value_lines(
                from,
                width,
                Style::default().fg(theme.danger),
            ));
            lines.extend(history_text_lines("to:", width, muted));
            lines.extend(history_value_lines(
                to,
                width,
                Style::default().fg(theme.success),
            ));
            lines
        }
        TaskHistoryKind::Added { value, .. } | TaskHistoryKind::Removed { value, .. } => {
            history_text_lines(value, width, muted)
        }
        TaskHistoryKind::TagAdded(tag) | TaskHistoryKind::TagRemoved(tag) => {
            history_tag_event_lines(app, tag, width, theme)
        }
    }
}

fn history_text_lines(value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap_hanging(value, width, width)
        .into_iter()
        .map(|line| Line::styled(line, style))
        .collect()
}

fn history_value_lines(value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let indent = "  ";
    let value = if value.is_empty() { "—" } else { value };
    wrap_hanging(
        value,
        width.saturating_sub(indent.len()).max(1),
        width.saturating_sub(indent.len()).max(1),
    )
    .into_iter()
    .map(|value| {
        Line::from(vec![
            Span::styled(indent, style),
            Span::styled(value, style),
        ])
    })
    .collect()
}

fn history_tag_event_lines(
    app: &App,
    tag: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let style = app
        .board
        .tag_by_name(tag)
        .map(tag_style)
        .unwrap_or_else(|| Style::default().fg(theme.text).bg(theme.border));
    let token = format!(" {tag} ");
    let mut remaining = token.as_str();
    let mut lines = Vec::new();
    while !remaining.is_empty() {
        let split = width_prefix_end(remaining, width.max(1));
        lines.push(Line::from(Span::styled(
            remaining[..split].to_owned(),
            style,
        )));
        remaining = &remaining[split..];
    }
    lines
}

fn checklist_section_header(theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::raw(""),
        Line::styled(
            "  Checklist",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn checklist_item_times(
    app: &App,
    task: &Task,
    index: usize,
    item: &ChecklistItem,
) -> (DateTime<Utc>, Option<DateTime<Utc>>) {
    let events = app
        .task_history
        .get(&task.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let field = format!("checklist item {}", index + 1);
    let added_at = item
        .added_at
        .or_else(|| {
            events.iter().rev().find_map(|event| match &event.kind {
                TaskHistoryKind::Added {
                    field: event_field,
                    value,
                } if value == &item.text => Some(event.at),
                _ => None,
            })
        })
        .or_else(|| {
            events.iter().rev().find_map(|event| match &event.kind {
                TaskHistoryKind::Added {
                    field: event_field, ..
                } if event_field == &field => Some(event.at),
                _ => None,
            })
        })
        .unwrap_or(task.created_at);
    let completed_at = item.completed.then(|| {
        item.completed_at
            .or_else(|| {
                events.iter().rev().find_map(|event| match &event.kind {
                    TaskHistoryKind::Changed { field, to, .. }
                        if to == "complete"
                            && checklist_status_item(field).as_deref()
                                == Some(item.text.as_str()) =>
                    {
                        Some(event.at)
                    }
                    _ => None,
                })
            })
            .unwrap_or(added_at)
    });
    (added_at, completed_at)
}

fn checklist_item_lines(
    item: &ChecklistItem,
    added_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let marker = if item.completed { "[x]" } else { "[ ]" };
    let prefix = format!("{} {marker} ", if selected { "  ›" } else { "   " });
    let indent = "        ";
    let style = task_detail_style(selected, theme);
    let mut lines = wrap_hanging(
        &item.text,
        width.saturating_sub(Span::raw(&prefix).width()).max(1),
        width.saturating_sub(Span::raw(indent).width()).max(1),
    )
    .into_iter()
    .enumerate()
    .map(|(index, value)| {
        Line::from(vec![
            Span::styled(
                if index == 0 {
                    prefix.clone()
                } else {
                    indent.into()
                },
                style,
            ),
            Span::styled(value, style),
        ])
    })
    .collect::<Vec<_>>();
    let now = Utc::now();
    let mut timing = format!("      Added {}", relative_time(added_at, now));
    if let Some(completed_at) = completed_at {
        timing.push_str(&format!(
            " · Completed {}",
            relative_time(completed_at, now)
        ));
    }
    lines.push(Line::styled(timing, Style::default().fg(theme.muted)));
    lines
}

fn relative_time(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(at).max(Duration::zero());
    if elapsed < Duration::minutes(1) {
        return "just now".into();
    }
    let (count, unit) = if elapsed < Duration::hours(1) {
        (elapsed.num_minutes(), "minute")
    } else if elapsed < Duration::days(1) {
        (elapsed.num_hours(), "hour")
    } else if elapsed < Duration::days(30) {
        (elapsed.num_days(), "day")
    } else if elapsed < Duration::days(365) {
        (elapsed.num_days() / 30, "month")
    } else {
        (elapsed.num_days() / 365, "year")
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

fn task_detail_text_lines(
    label: &str,
    label_width: usize,
    value: &str,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let style = task_detail_style(selected, theme);
    let prefix = format!(
        "{} {label:<label_width$}  ",
        if selected { "›" } else { " " }
    );
    let indent = " ".repeat(Span::raw(&prefix).width());
    let wrapped = wrap_hanging(
        value,
        width.saturating_sub(Span::raw(&prefix).width()).max(1),
        width.saturating_sub(Span::raw(&indent).width()).max(1),
    );
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 {
                        prefix.clone()
                    } else {
                        indent.clone()
                    },
                    style,
                ),
                Span::styled(value, style),
            ])
        })
        .collect()
}

fn task_detail_tag_lines(
    label: &str,
    label_width: usize,
    tags: &[String],
    selected: bool,
    width: usize,
    app: &App,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let label_style = task_detail_style(selected, theme);
    let prefix = format!(
        "{} {label:<label_width$}  ",
        if selected { "›" } else { " " }
    );
    let indent = " ".repeat(Span::raw(&prefix).width());
    let mut lines = Vec::new();
    let mut spans = vec![Span::styled(prefix.clone(), label_style)];
    let mut used = Span::raw(&prefix).width();
    let mut has_tag = false;

    for name in tags {
        let token = format!(" {name} ");
        let token_width = Span::raw(&token).width();
        if has_tag && used + 1 + token_width > width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent.clone(), label_style)];
            used = Span::raw(&indent).width();
            has_tag = false;
        }
        if has_tag {
            spans.push(Span::raw(" "));
            used += 1;
        }
        if used >= width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent.clone(), label_style)];
            used = Span::raw(&indent).width();
        }

        let style = app
            .board
            .tag_by_name(name)
            .map(tag_style)
            .unwrap_or_else(|| Style::default().fg(theme.text).bg(theme.border));
        let mut remaining = token.as_str();
        while !remaining.is_empty() {
            let available = width.saturating_sub(used).max(1);
            let split = width_prefix_end(remaining, available);
            spans.push(Span::styled(remaining[..split].to_owned(), style));
            used += Span::raw(&remaining[..split]).width();
            remaining = &remaining[split..];
            if !remaining.is_empty() {
                lines.push(Line::from(spans));
                spans = vec![Span::styled(indent.clone(), label_style)];
                used = Span::raw(&indent).width();
            }
        }
        has_tag = true;
    }
    lines.push(Line::from(spans));
    lines
}

fn task_detail_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    }
}

fn wrap_hanging(value: &str, first_width: usize, continuation_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut limit = first_width.max(1);

    for (paragraph_index, paragraph) in value.split('\n').enumerate() {
        if paragraph_index > 0 {
            lines.push(std::mem::take(&mut current));
            limit = continuation_width.max(1);
        }
        for mut word in paragraph.split_whitespace() {
            loop {
                let separator = usize::from(!current.is_empty());
                if Span::raw(&current).width() + separator + Span::raw(word).width() <= limit {
                    if separator == 1 {
                        current.push(' ');
                    }
                    current.push_str(word);
                    break;
                }
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    limit = continuation_width.max(1);
                    continue;
                }
                let split = width_prefix_end(word, limit);
                lines.push(word[..split].to_owned());
                word = &word[split..];
                limit = continuation_width.max(1);
                if word.is_empty() {
                    break;
                }
            }
        }
    }
    lines.push(current);
    lines
}

fn width_prefix_end(value: &str, max_width: usize) -> usize {
    let mut width = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        let character_width = Span::raw(character.to_string()).width();
        if end != 0 && width + character_width > max_width {
            break;
        }
        width += character_width;
        end = next;
        if width >= max_width {
            break;
        }
    }
    end.max(value.chars().next().map_or(0, char::len_utf8))
}

fn draw_column_details(frame: &mut Frame, app: &App, cursor: usize, theme: &Theme) {
    let area = centered(frame.area(), 58, 10);
    let column = app.current_column();
    let values = [
        format!("Name: {}", column.title),
        format!("Tasks: {}", column.tasks.len()),
    ];
    let lines = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let selected = index == cursor;
            Line::styled(
                format!("{} {value}", if selected { "›" } else { " " }),
                if selected {
                    Style::default()
                        .fg(theme.accent)
                        .bg(theme.selected_background)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                },
            )
        })
        .chain([
            Line::raw(""),
            Line::styled(
                format!("  Column id: {}", column.id),
                Style::default().fg(theme.muted),
            ),
        ])
        .collect();
    render_modal(
        frame,
        area,
        " Column details ",
        lines,
        "arrows/hjkl select · Enter/e/r rename · Esc/q close · ? help",
        theme,
    );
}

fn draw_delete_confirmation(
    frame: &mut Frame,
    app: &App,
    state: &DeleteConfirmation,
    theme: &Theme,
) {
    let area = centered_fixed(frame.area(), 66, 11);
    let (question, consequence) = match state.target {
        DeleteTarget::Column { index } => {
            let column = &app.board.columns[index];
            let count = column.tasks.len();
            (
                format!("Delete column {:?}?", column.title),
                (count > 0).then(|| {
                    let prior = &app.board.columns[index - 1];
                    format!(
                        "Its {count} task{} will be moved to the prior column {:?}.",
                        if count == 1 { "" } else { "s" },
                        prior.title
                    )
                }),
            )
        }
        DeleteTarget::Task { column, task } => {
            let task = &app.board.columns[column].tasks[task];
            (
                format!("Delete task {:?}?", task.title),
                Some("The task and all of its details will be removed.".into()),
            )
        }
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm deletion ")
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.danger))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [message_area, buttons_area, hint_area] = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(inner);

    let mut message = vec![Line::styled(
        format!("  {question}"),
        Style::default()
            .fg(theme.danger)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(consequence) = consequence {
        message.push(Line::styled(
            format!("  {consequence}"),
            Style::default().fg(theme.text),
        ));
    }
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().bg(theme.background))
            .wrap(Wrap { trim: false }),
        message_area,
    );

    let button_width = 14;
    let gap = 3;
    let group_width = (button_width * 2 + gap).min(buttons_area.width);
    let group = Rect::new(
        buttons_area.x + buttons_area.width.saturating_sub(group_width) / 2,
        buttons_area.y,
        group_width,
        buttons_area.height,
    );
    let [cancel_area, _, delete_area] = Layout::horizontal([
        Constraint::Length(button_width),
        Constraint::Length(gap),
        Constraint::Length(button_width),
    ])
    .areas(group);
    draw_confirmation_button(
        frame,
        cancel_area,
        "Cancel",
        state.choice == DeleteChoice::Cancel,
        theme.accent,
        theme,
    );
    draw_confirmation_button(
        frame,
        delete_area,
        "Delete",
        state.choice == DeleteChoice::Delete,
        theme.danger,
        theme,
    );
    frame.render_widget(
        Paragraph::new("←/→ or h/l select · Enter activate · y delete · n/Esc/q cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn draw_confirmation_button(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    selected: bool,
    selected_color: Color,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if selected {
            selected_color
        } else {
            theme.border
        }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(if selected {
            format!("> {label} <")
        } else {
            label.into()
        })
        .alignment(Alignment::Center)
        .style(
            Style::default()
                .fg(if selected { selected_color } else { theme.text })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        inner,
    );
}

fn text_editor_lines(
    editor: &TextEditor,
    first_prefix: &'static str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (cursor_row, cursor_column) = editor.cursor();
    editor
        .lines()
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans = Vec::new();
            if row == 0 && !first_prefix.is_empty() {
                spans.push(Span::styled(first_prefix, Style::default().fg(theme.muted)));
            }
            if row == cursor_row {
                let cursor_byte = line
                    .char_indices()
                    .nth(cursor_column)
                    .map_or(line.len(), |(byte, _)| byte);
                let cursor_end = line[cursor_byte..]
                    .char_indices()
                    .nth(1)
                    .map_or(line.len(), |(byte, _)| cursor_byte + byte);
                spans.push(Span::styled(
                    line[..cursor_byte].to_owned(),
                    Style::default().fg(theme.text),
                ));
                spans.push(Span::styled(
                    if cursor_byte == line.len() {
                        " ".to_owned()
                    } else {
                        line[cursor_byte..cursor_end].to_owned()
                    },
                    Style::default()
                        .fg(theme.background)
                        .bg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    line[cursor_end..].to_owned(),
                    Style::default().fg(theme.text),
                ));
            } else {
                spans.push(Span::styled(line.clone(), Style::default().fg(theme.text)));
            }
            Line::from(spans)
        })
        .collect()
}

fn rendered_input_cursor_row(
    paragraph: &Paragraph<'_>,
    width: u16,
    height: u16,
    theme: &Theme,
) -> u16 {
    if width == 0 || height == 0 {
        return 0;
    }
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    paragraph.clone().render(area, &mut buffer);
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if cell.bg == theme.accent {
                return y;
            }
        }
    }
    0
}

fn draw_input(frame: &mut Frame, input: &crate::app::InputState, app: &App, theme: &Theme) {
    let frame_area = frame.area();
    let width = frame_area
        .width
        .saturating_mul(64)
        .saturating_div(100)
        .max(24)
        .min(frame_area.width)
        .max(1);
    let inner_width = width.saturating_sub(2).max(1);
    let text_width = inner_width.saturating_sub(3).max(1);
    let mut lines = text_editor_lines(&input.editor, "", theme);
    if let Some(status) = &app.status {
        lines.push(Line::styled(
            status.clone(),
            Style::default().fg(theme.danger),
        ));
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.text))
        .wrap(Wrap { trim: false });
    let content_height = usize_to_u16(paragraph.line_count(text_width));
    let height = content_height
        .saturating_add(4)
        .max(5)
        .min(frame_area.height.saturating_sub(2).max(1));
    let area = centered_fixed(frame_area, width, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", input.kind.title()))
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    let [content_row, _bottom_padding, hint_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [prefix_area, content_area, _right_padding] = Layout::horizontal([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(content_row);
    let cursor_row = rendered_input_cursor_row(&paragraph, text_width, content_height, theme);
    let scroll = if content_area.height == 0 {
        0
    } else {
        cursor_row
            .saturating_add(1)
            .saturating_sub(content_area.height)
    };
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new("> ").style(Style::default().fg(theme.muted)),
        prefix_area,
    );
    frame.render_widget(paragraph.scroll((scroll, 0)), content_area);
    frame.render_widget(
        Paragraph::new("Ctrl-G $EDITOR · Ctrl-/ keymap")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn draw_date_picker(frame: &mut Frame, picker: &crate::app::DatePickerState, theme: &Theme) {
    let area = centered_fixed(frame.area(), 84, 14);
    let first = NaiveDate::from_ymd_opt(picker.selected.year(), picker.selected.month(), 1)
        .expect("selected date always has a valid first day");
    let grid_start = first
        .checked_sub_signed(Duration::days(
            first.weekday().num_days_from_monday().into(),
        ))
        .unwrap_or(first);
    let today = Local::now().date_naive();
    let mut lines = vec![
        Line::styled(
            picker.selected.format("%B %Y").to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            " Mo  Tu  We  Th  Fr  Sa  Su ",
            Style::default().fg(theme.muted),
        ),
    ];
    for week in 0..6 {
        let mut days = Vec::new();
        for weekday in 0..7 {
            let offset = week * 7 + weekday;
            let date = grid_start
                .checked_add_signed(Duration::days(offset))
                .unwrap_or(grid_start);
            let mut style = if date.month() == picker.selected.month() {
                Style::default().fg(theme.text)
            } else {
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM)
            };
            if date == today {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if date == picker.selected {
                style = style
                    .fg(theme.accent)
                    .bg(theme.selected_background)
                    .add_modifier(Modifier::BOLD);
            }
            days.push(Span::styled(format!(" {:>2} ", date.day()), style));
        }
        lines.push(Line::from(days));
    }
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Due date picker ")
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(theme.background)),
        content_area,
    );
    frame.render_widget(
        Paragraph::new(
            "arrows/hjkl move · PgUp/PgDn month · t today · Enter set · d clear · Esc/q cancel · ? help",
        )
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn draw_tag_picker(
    frame: &mut Frame,
    app: &App,
    picker: &crate::app::TagPickerState,
    theme: &Theme,
) {
    let area = centered_fixed(frame.area(), 76, 9);
    let current = app.current_task().tags.clone();
    let available = app.available_tag_names();
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Tag picker ")
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_tag_picker_row(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        "Current",
        &current,
        picker.row == TagPickerRow::Current,
        picker.index,
        false,
        app,
        theme,
    );
    render_tag_picker_row(
        frame,
        Rect::new(inner.x, inner.y.saturating_add(2), inner.width, 1),
        "Available",
        &available,
        picker.row == TagPickerRow::Available,
        picker.index,
        true,
        app,
        theme,
    );
    frame.render_widget(
        Paragraph::new("arrows/hjkl select · Enter add/remove/create · Esc close · ? help")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
}

#[allow(clippy::too_many_arguments)]
fn render_tag_picker_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    names: &[String],
    active: bool,
    selected_index: usize,
    include_new: bool,
    app: &App,
    theme: &Theme,
) {
    let label_width = area.width.min(11);
    frame.render_widget(
        Paragraph::new(label).style(
            Style::default()
                .fg(if active { theme.accent } else { theme.text })
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y, label_width, 1),
    );
    let items_area = Rect::new(
        area.x.saturating_add(label_width),
        area.y,
        area.width.saturating_sub(label_width),
        1,
    );
    let (line, selected_range) =
        tag_picker_items(names, active, selected_index, include_new, app, theme);
    let scroll = selected_range
        .map(|(_, end)| end.saturating_sub(usize::from(items_area.width)))
        .unwrap_or(0);
    frame.render_widget(
        Paragraph::new(line).scroll((0, usize_to_u16(scroll))),
        items_area,
    );
}

fn tag_picker_items(
    names: &[String],
    active: bool,
    selected_index: usize,
    include_new: bool,
    app: &App,
    theme: &Theme,
) -> (Line<'static>, Option<(usize, usize)>) {
    let mut spans = Vec::new();
    let mut width = 0;
    let mut selected_range = None;
    if names.is_empty() && !include_new {
        return (
            Line::styled("(none)", Style::default().fg(theme.muted)),
            None,
        );
    }
    for (index, name) in names.iter().enumerate() {
        let selected = active && selected_index == index;
        let start = width;
        let open = Span::styled(
            if selected { ">" } else { " " },
            Style::default().fg(theme.accent),
        );
        width += open.width();
        spans.push(open);
        let style = app
            .board
            .tag_by_name(name)
            .map(tag_style)
            .unwrap_or_else(|| Style::default().fg(theme.text).bg(theme.border));
        let tag = Span::styled(format!(" {name} "), style);
        width += tag.width();
        spans.push(tag);
        let close = Span::styled(
            if selected { "<" } else { " " },
            Style::default().fg(theme.accent),
        );
        width += close.width();
        spans.push(close);
        if selected {
            selected_range = Some((start, width));
        }
    }
    if include_new {
        let index = names.len();
        let selected = active && selected_index == index;
        let start = width;
        let open = Span::styled(
            if selected { ">" } else { " " },
            Style::default().fg(theme.accent),
        );
        width += open.width();
        spans.push(open);
        let new_tag = Span::styled(
            " New Tag ",
            Style::default()
                .fg(if selected { theme.accent } else { theme.text })
                .bg(if selected {
                    theme.selected_background
                } else {
                    Color::Reset
                })
                .add_modifier(Modifier::BOLD),
        );
        width += new_tag.width();
        spans.push(new_tag);
        let close = Span::styled(
            if selected { "<" } else { " " },
            Style::default().fg(theme.accent),
        );
        width += close.width();
        spans.push(close);
        if selected {
            selected_range = Some((start, width));
        }
    }
    (Line::from(spans), selected_range)
}

fn draw_new_tag(frame: &mut Frame, app: &App, state: &crate::app::NewTagState, theme: &Theme) {
    let area = centered_fixed(frame.area(), 72, 13);
    let name_selected = state.field == NewTagField::Name;
    let color_selected = state.field == NewTagField::Color;
    let color = TAG_COLOR_PALETTE[state.color_index];
    let name = state.name.text();
    let mut swatches = vec![Span::styled("Palette  ", Style::default().fg(theme.text))];
    for (index, color) in TAG_COLOR_PALETTE.iter().enumerate() {
        let selected = color_selected && index == state.color_index;
        swatches.push(Span::styled(
            if selected { ">" } else { " " },
            Style::default().fg(theme.accent),
        ));
        let (red, green, blue) = tag_rgb(color).expect("palette colors are valid");
        swatches.push(Span::styled(
            "  ",
            Style::default().bg(Color::Rgb(red, green, blue)),
        ));
        swatches.push(Span::styled(
            if selected { "<" } else { " " },
            Style::default().fg(theme.accent),
        ));
    }
    let preview = TagDefinition {
        id: 0,
        name: name.clone(),
        color: color.into(),
    };
    let name_line = if name_selected {
        text_editor_lines(&state.name, "› Name: ", theme)
            .into_iter()
            .next()
            .unwrap_or_default()
    } else {
        Line::styled(format!("  Name: {name}"), Style::default().fg(theme.text))
    };
    let mut lines = vec![
        name_line,
        Line::raw(""),
        Line::styled(
            format!("{} Color: {color}", if color_selected { "›" } else { " " }),
            if color_selected {
                Style::default().fg(theme.accent)
            } else {
                Style::default().fg(theme.text)
            },
        ),
        Line::from(swatches),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Preview  ", Style::default().fg(theme.text)),
            Span::styled(
                format!(" {} ", if name.is_empty() { "tag" } else { &name }),
                tag_style(&preview),
            ),
        ]),
        Line::raw(""),
    ];
    if let Some(status) = &app.status {
        lines.push(Line::styled(
            status.clone(),
            Style::default().fg(theme.danger),
        ));
    }
    render_modal(
        frame,
        area,
        " New tag ",
        lines,
        "Tab/↑↓ fields · h/l color · Ctrl-G $EDITOR · Ctrl-/ keymap",
        theme,
    );
}

fn draw_help(frame: &mut Frame, theme: &Theme) {
    let area = centered(frame.area(), 90, 24);
    let heading = |text: &'static str| {
        Line::styled(
            text,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    };
    let key = |keys: &'static str, action: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<16}"), Style::default().fg(theme.text)),
            Span::styled(action, Style::default().fg(theme.muted)),
        ])
    };
    let left = vec![
        heading("GLOBAL"),
        key("q", "close/quit; inserts in text fields"),
        key("Ctrl-C", "quit tdo immediately"),
        key("? / Ctrl-/", "open keymap; Ctrl-/ while typing"),
        Line::raw(""),
        heading("BOARD"),
        key("arrows / hjkl", "navigate headers and task cards"),
        if theme.mouse_enabled {
            key("Click / double", "select / open a task card")
        } else {
            Line::raw("")
        },
        key("1-9", "jump to a column"),
        key("Enter", "open column or task details"),
        key("a", "add a task"),
        key("C", "add a column"),
        key("r", "rename the selected column"),
        key("D", "delete the selected column or task"),
        key("m", "enter MOVE mode"),
        heading("DATE PICKER"),
        key("arrows / hjkl", "select a day"),
        key("PgUp / PgDn", "change month"),
        key("t", "select today"),
        key("Enter", "confirm due date"),
        key("d / Delete", "clear due date"),
        key("Esc", "cancel date change"),
    ];
    let right = vec![
        heading("MOVE MODE"),
        key("arrows / hjkl", "reposition the task"),
        key("Enter / m", "confirm the move"),
        key("Esc", "cancel the move"),
        heading("DETAILS"),
        key("arrows / hjkl", "select a field"),
        key("Enter / e", "activate or edit the field"),
        if theme.mouse_enabled {
            key("Space / click", "toggle a checklist item")
        } else {
            key("Space", "toggle a checklist item")
        },
        key("a / d", "add / delete checklist item"),
        if theme.mouse_enabled {
            key("^u / ^d / Wheel", "scroll the details page")
        } else {
            key("^u / ^d", "scroll the details page")
        },
        if theme.mouse_enabled {
            key("[×] / Esc", "close details")
        } else {
            key("Esc", "close details")
        },
        heading("INPUT"),
        key("arrows/Home/End", "move the text cursor"),
        key("Backspace/Delete", "delete around the cursor"),
        key("Ctrl-U / Ctrl-R", "undo / redo"),
        key("Ctrl-G", "edit with $EDITOR"),
        key("Ctrl-/", "open this keymap"),
        heading("TAG PICKER"),
        key("arrows / hjkl", "select a tag or field"),
        key("Enter", "add, remove, or create"),
        key("Tab / ↑↓", "switch new-tag field"),
        key("h / l", "choose tag color"),
        key("Esc", "cancel or close"),
    ];
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" tdo · modal todo list · keymap · Esc to close ")
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(content_area);
    frame.render_widget(Paragraph::new(left), columns[0]);
    frame.render_widget(Paragraph::new(right), columns[1]);
    frame.render_widget(
        Paragraph::new("Esc/q close · Ctrl-C quit")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn draw_text_input_help(frame: &mut Frame, theme: &Theme) {
    let key = |keys: &'static str, action: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<20}"), Style::default().fg(theme.text)),
            Span::styled(action, Style::default().fg(theme.muted)),
        ])
    };
    let lines = vec![
        key("arrows / Ctrl-B/F/P/N", "move the cursor"),
        key("Home/End · Ctrl-A/E", "move to line edge"),
        key("Ctrl-← / Ctrl-→", "move by word"),
        key("Backspace / Delete", "delete around the cursor"),
        key("Ctrl-U / Ctrl-R", "undo / redo"),
        key("Ctrl-G", "edit with $EDITOR"),
    ];
    let area = centered_fixed(frame.area(), 62, 9);
    render_modal(
        frame,
        area,
        " Text input · keymap ",
        lines,
        "Esc/q close keymap · Ctrl-C quit",
        theme,
    );
}

fn render_task_detail_modal(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    hint: &str,
    more_below: bool,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Task details ")
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let [content_area, more_area, hint_area] = task_detail_modal_areas(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.background))
            .wrap(Wrap { trim: false }),
        content_area,
    );
    if more_below {
        frame.render_widget(
            Paragraph::new("↓ (more)")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted).bg(theme.background)),
            more_area,
        );
    }
    frame.render_widget(
        Paragraph::new(hint)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn render_modal(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    hint: &str,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_alignment(Alignment::Center)
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.background))
            .wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new(hint)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted).bg(theme.background)),
        hint_area,
    );
}

fn task_card_lines(app: &App, task: &Task, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let width = usize::from(width).max(1);
    let mut lines = wrap_hanging(&task.title, width, width)
        .into_iter()
        .map(|line| {
            Line::styled(
                line,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    if !task.tags.is_empty() {
        lines.extend(task_card_tag_lines(
            &task.tags,
            width,
            Style::default().fg(theme.muted),
            |name| {
                app.board
                    .tag_by_name(name)
                    .map(tag_style)
                    .unwrap_or_else(|| Style::default().fg(theme.text).bg(theme.border))
            },
        ));
    }
    if let Some(progress) = task_checklist_progress(task) {
        lines.extend(task_card_bullet_lines(
            &progress,
            width,
            Style::default().fg(theme.muted),
        ));
    }
    if let Some(date) = task.due_date {
        lines.extend(task_card_bullet_lines(
            &format!("due {}", date.format("%Y-%m-%d")),
            width,
            Style::default().fg(theme.muted),
        ));
    }
    lines
}

fn task_checklist_progress(task: &Task) -> Option<String> {
    let total = task.checklist.len();
    if total == 0 {
        return None;
    }
    let completed = task.checklist.iter().filter(|item| item.completed).count();
    Some(format!(
        "[{}] {completed} / {total}",
        if completed == total { "x" } else { " " }
    ))
}

fn task_card_bullet_lines(value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let prefix = "  - ";
    let indent = "    ";
    wrap_hanging(
        value,
        width.saturating_sub(Span::raw(prefix).width()).max(1),
        width.saturating_sub(Span::raw(indent).width()).max(1),
    )
    .into_iter()
    .enumerate()
    .map(|(index, value)| {
        Line::from(vec![
            Span::styled(if index == 0 { prefix } else { indent }, style),
            Span::styled(value, style),
        ])
    })
    .collect()
}

fn task_card_tag_lines<F>(
    tags: &[String],
    width: usize,
    prefix_style: Style,
    mut style_for: F,
) -> Vec<Line<'static>>
where
    F: FnMut(&str) -> Style,
{
    let prefix = "  - ";
    let indent = "    ";
    let mut lines = Vec::new();
    let mut spans = vec![Span::styled(prefix, prefix_style)];
    let mut used = Span::raw(prefix).width();
    let mut has_tag = false;

    for name in tags {
        let token = format!(" {name} ");
        let token_width = Span::raw(&token).width();
        if has_tag && used + 1 + token_width > width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent, prefix_style)];
            used = Span::raw(indent).width();
            has_tag = false;
        }
        if has_tag {
            spans.push(Span::raw(" "));
            used += 1;
        }
        if used >= width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent, prefix_style)];
            used = Span::raw(indent).width();
        }

        let style = style_for(name);
        let mut remaining = token.as_str();
        while !remaining.is_empty() {
            let available = width.saturating_sub(used).max(1);
            let split = width_prefix_end(remaining, available);
            spans.push(Span::styled(remaining[..split].to_owned(), style));
            used += Span::raw(&remaining[..split]).width();
            remaining = &remaining[split..];
            if !remaining.is_empty() {
                lines.push(Line::from(spans));
                spans = vec![Span::styled(indent, prefix_style)];
                used = Span::raw(indent).width();
            }
        }
        has_tag = true;
    }
    lines.push(Line::from(spans));
    lines
}

fn tag_style(tag: &TagDefinition) -> Style {
    let (red, green, blue) = tag_rgb(&tag.color).unwrap_or((127, 132, 142));
    Style::default()
        .fg(contrasting_text_color(red, green, blue))
        .bg(Color::Rgb(red, green, blue))
}

fn tag_rgb(value: &str) -> Option<(u8, u8, u8)> {
    if value.len() != 7 || !value.starts_with('#') {
        return None;
    }
    Some((
        u8::from_str_radix(&value[1..3], 16).ok()?,
        u8::from_str_radix(&value[3..5], 16).ok()?,
        u8::from_str_radix(&value[5..7], 16).ok()?,
    ))
}

fn contrasting_text_color(red: u8, green: u8, blue: u8) -> Color {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
    let white_contrast = 1.05 / (luminance + 0.05);
    let black_contrast = (luminance + 0.05) / 0.05;
    if black_contrast >= white_contrast {
        Color::Black
    } else {
        Color::White
    }
}

fn visible_lanes(area: Rect, app: &App) -> Vec<(usize, Rect)> {
    if area.width == 0 || area.height == 0 || app.board.columns.is_empty() {
        return Vec::new();
    }
    let visible_count =
        usize::from((area.width / MIN_COLUMN_WIDTH).max(1)).min(app.board.columns.len());
    let start = if app.selected_column >= visible_count {
        app.selected_column + 1 - visible_count
    } else {
        0
    };
    let constraints = vec![Constraint::Ratio(1, visible_count as u32); visible_count];
    Layout::horizontal(constraints)
        .split(area)
        .iter()
        .copied()
        .enumerate()
        .map(|(visible_index, lane)| (start + visible_index, lane))
        .collect()
}

fn lane_regions(lane: Rect) -> (Rect, Rect) {
    let header_height = lane.height.min(3);
    (
        Rect::new(lane.x, lane.y, lane.width, header_height),
        Rect::new(
            lane.x,
            lane.y.saturating_add(header_height),
            lane.width,
            lane.height.saturating_sub(header_height),
        ),
    )
}

fn visible_cards(area: Rect, app: &App, column_index: usize) -> Vec<(usize, Rect)> {
    column_card_layout(area, app, column_index).cards
}

fn column_card_layout(area: Rect, app: &App, column_index: usize) -> ColumnCardLayout {
    let tasks = &app.board.columns[column_index].tasks;
    if tasks.is_empty() || area.width == 0 || area.height == 0 {
        return ColumnCardLayout {
            start: 0,
            cards: Vec::new(),
            hidden_above: 0,
            hidden_below: tasks.len(),
        };
    }
    let selected = (column_index == app.selected_column
        && app.column_scroll_follows_cursor(column_index))
    .then_some(app.selected_task)
    .flatten();
    let moving = matches!(app.mode, Mode::Moving(_));
    let mut start = app
        .column_scroll(column_index)
        .min(tasks.len().saturating_sub(1));
    if let Some(selected) = selected.filter(|selected| *selected < tasks.len()) {
        if selected < start {
            start = selected;
        }
        while start < selected {
            let layout = column_cards_from_start(area, tasks, start, selected, moving);
            let required = task_card_height(&tasks[selected], area.width, moving);
            if layout
                .cards
                .iter()
                .find(|(index, _)| *index == selected)
                .is_some_and(|(_, rect)| rect.height >= required)
            {
                break;
            }
            start += 1;
        }
    }
    column_cards_from_start(area, tasks, start, selected.unwrap_or(usize::MAX), moving)
}

fn column_cards_from_start(
    area: Rect,
    tasks: &[Task],
    start: usize,
    selected: usize,
    moving: bool,
) -> ColumnCardLayout {
    let hidden_above = start.min(tasks.len());
    let top_rows = u16::from(hidden_above > 0 && area.height > 0);
    let without_bottom = Rect::new(
        area.x,
        area.y.saturating_add(top_rows),
        area.width,
        area.height.saturating_sub(top_rows),
    );
    let (mut cards, mut hidden_below) =
        layout_card_rects(without_bottom, tasks, start, selected, moving);
    if hidden_below > 0 && without_bottom.height > 0 {
        let with_bottom = Rect::new(
            without_bottom.x,
            without_bottom.y,
            without_bottom.width,
            without_bottom.height.saturating_sub(1),
        );
        (cards, hidden_below) = layout_card_rects(with_bottom, tasks, start, selected, moving);
    }
    ColumnCardLayout {
        start,
        cards,
        hidden_above,
        hidden_below,
    }
}

fn layout_card_rects(
    area: Rect,
    tasks: &[Task],
    start: usize,
    selected: usize,
    moving: bool,
) -> (Vec<(usize, Rect)>, usize) {
    let mut cards = Vec::new();
    let mut y = area.y;
    for (task_index, task) in tasks.iter().enumerate().skip(start) {
        let remaining = area.bottom().saturating_sub(y);
        if remaining == 0 {
            break;
        }
        let required = task_card_height(task, area.width, moving && selected == task_index);
        let height = required.min(remaining);
        cards.push((task_index, Rect::new(area.x, y, area.width, height)));
        if height < required {
            break;
        }
        y = y
            .saturating_add(height)
            .saturating_add(TASK_CARD_GAP)
            .min(area.bottom());
    }
    let hidden_below = cards
        .last()
        .map(|(index, _)| tasks.len().saturating_sub(index.saturating_add(1)))
        .unwrap_or_else(|| tasks.len().saturating_sub(start));
    (cards, hidden_below)
}

fn task_card_height(task: &Task, width: u16, moving: bool) -> u16 {
    let content_width = usize::from(
        width
            .saturating_sub(2)
            .saturating_sub(TASK_CARD_RIGHT_PADDING)
            .max(1),
    );
    let mut lines = wrap_hanging(&task.title, content_width, content_width)
        .len()
        .max(1);
    if !task.tags.is_empty() {
        lines += task_card_tag_lines(&task.tags, content_width, Style::default(), |_| {
            Style::default()
        })
        .len();
    }
    if let Some(progress) = task_checklist_progress(task) {
        lines += task_card_bullet_lines(&progress, content_width, Style::default()).len();
    }
    if let Some(date) = task.due_date {
        lines += task_card_bullet_lines(
            &format!("due {}", date.format("%Y-%m-%d")),
            content_width,
            Style::default(),
        )
        .len();
    }
    usize_to_u16(lines.saturating_add(usize::from(moving) * 2))
}

fn usize_to_u16(value: usize) -> u16 {
    value.min(usize::from(u16::MAX)) as u16
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

fn modal_content_area(area: Rect) -> Rect {
    task_detail_modal_areas(area)[0]
}

fn task_detail_modal_areas(area: Rect) -> [Rect; 3] {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner)
}

fn task_details_close_area(area: Rect) -> Rect {
    let width = 3.min(area.width.saturating_sub(2));
    Rect::new(
        area.right().saturating_sub(width.saturating_add(1)),
        area.y,
        width,
        1,
    )
}

fn centered(area: Rect, width_percent: u16, preferred_height: u16) -> Rect {
    let width = area
        .width
        .saturating_mul(width_percent)
        .saturating_div(100)
        .max(1);
    let height = preferred_height.min(area.height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn centered_fixed(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let width = preferred_width.min(area.width).max(1);
    let height = preferred_height.min(area.height).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn parse_color(value: &str) -> Result<Color> {
    let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let color = match normalized.as_str() {
        "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "orange" => Color::Indexed(208),
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "dark_gray" | "dark_grey" => Color::DarkGray,
        "light_red" => Color::LightRed,
        "light_green" => Color::LightGreen,
        "light_yellow" => Color::LightYellow,
        "light_blue" => Color::LightBlue,
        "light_magenta" => Color::LightMagenta,
        "light_cyan" => Color::LightCyan,
        "white" => Color::White,
        value if value.starts_with('#') && value.len() == 7 => {
            let red = u8::from_str_radix(&value[1..3], 16);
            let green = u8::from_str_radix(&value[3..5], 16);
            let blue = u8::from_str_radix(&value[5..7], 16);
            match (red, green, blue) {
                (Ok(red), Ok(green), Ok(blue)) => Color::Rgb(red, green, blue),
                _ => bail!("invalid theme color {value:?}"),
            }
        }
        _ => bail!("unknown theme color {value:?}"),
    };
    Ok(color)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use crate::history::{TaskHistoryEvent, TaskHistoryKind};
    use crate::{app::App, config::ThemeConfig, model::Board};

    use super::*;

    #[test]
    fn parses_named_and_rgb_colors() {
        assert_eq!(parse_color("dark-gray").unwrap(), Color::DarkGray);
        assert_eq!(parse_color("orange").unwrap(), Color::Indexed(208));
        assert_eq!(
            parse_color("#12abEF").unwrap(),
            Color::Rgb(0x12, 0xab, 0xef)
        );
    }

    #[test]
    fn board_footer_omits_navigation_and_enter_hints() {
        let app = App::new(Board::default());
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(100, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let footer = buffer_row_text(terminal.backend().buffer(), 100, 9);
        assert!(footer.contains("BOARD mode · a add task"));
        assert!(!footer.contains("navigate"));
        assert!(!footer.contains("enter"));
    }

    #[test]
    fn board_background_is_black_in_every_cell() {
        let app = App::new(Board::default());
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.bg == TUI_BACKGROUND)
        );
    }

    #[test]
    fn partial_cards_render_and_selection_scrolls_them_fully_into_view() {
        let mut board = Board::default();
        board.add_task(0, "short".into());
        board.add_task(0, "x".repeat(45));
        let mut app = App::new(board);
        let area = Rect::new(0, 0, 20, 4);
        let tall_height = task_card_height(&app.board.columns[0].tasks[1], area.width, false);
        assert_eq!(tall_height, 3);

        let layout = column_card_layout(area, &app, 0);
        assert_eq!(layout.cards.len(), 2);
        assert_eq!(layout.cards[1].0, 1);
        assert!(layout.cards[1].1.height < tall_height);

        app.select_target(0, Some(1));
        let layout = column_card_layout(area, &app, 0);
        assert_eq!(layout.start, 1);
        assert_eq!(layout.hidden_above, 1);
        assert_eq!(layout.cards, vec![(1, Rect::new(0, 1, 20, tall_height))]);
    }

    #[test]
    fn columns_keep_independent_scroll_positions_and_count_hidden_cards() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        for index in 0..8 {
            board.add_task(0, format!("Task {index}"));
        }
        let mut app = App::new(board);
        app.selected_task = Some(7);
        let viewport = Rect::new(0, 0, 60, 12);

        prepare_board_scrolls(viewport, &mut app);
        let saved_start = app.column_scroll(0);
        assert!(saved_start > 0);
        app.select_target(1, None);
        prepare_board_scrolls(viewport, &mut app);
        assert_eq!(app.column_scroll(0), saved_start);

        let lane = visible_lanes(Rect::new(0, 0, 60, 11), &app)[0].1;
        let inner = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .inner(lane_regions(lane).1);
        let saved_layout = column_card_layout(inner, &app, 0);
        assert_eq!(saved_layout.start, saved_start);
        assert_eq!(saved_layout.hidden_above, saved_start);

        app.scroll_column(0, -100);
        let top_layout = column_card_layout(inner, &app, 0);
        assert_eq!(top_layout.hidden_above, 0);
        assert!(top_layout.hidden_below > 0);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let screen = buffer_text(terminal.backend().buffer(), 60, 12);
        assert!(screen.contains(&format!("↓ ({} more)", top_layout.hidden_below)));
        assert!(!screen.contains("↑ ("));

        app.scroll_column(0, 100);
        let bottom_layout = column_card_layout(inner, &app, 0);
        assert_eq!(bottom_layout.hidden_above, 7);
        assert_eq!(bottom_layout.hidden_below, 0);

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let screen = buffer_text(terminal.backend().buffer(), 60, 12);
        assert!(screen.contains("↑ (7 more)"));
        assert!(!screen.contains("↓ ("));
    }

    #[test]
    fn returning_to_a_column_restores_its_cursor_without_scrolling_again() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        for column in 0..2 {
            for index in 0..8 {
                board.add_task(column, format!("Task {column}-{index}"));
            }
        }
        let mut app = App::new(board);
        let viewport = Rect::new(0, 0, 60, 12);
        app.selected_task = Some(7);
        prepare_board_scrolls(viewport, &mut app);
        let first_column_scroll = app.column_scroll(0);
        assert!(first_column_scroll > 0);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        for _ in 0..3 {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        prepare_board_scrolls(viewport, &mut app);
        assert_eq!(app.selected_task, Some(2));

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.selected_task, Some(7));
        prepare_board_scrolls(viewport, &mut app);
        assert_eq!(app.column_scroll(0), first_column_scroll);
    }

    #[test]
    fn mouse_wheel_scrolls_the_hovered_column_and_only_the_details_window() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        for column in 0..2 {
            for index in 0..8 {
                board.add_task(column, format!("Task {column}-{index}"));
            }
        }
        for index in 0..20 {
            board.columns[0].tasks[0]
                .checklist
                .push(ChecklistItem::new(format!("Item {index}")));
        }
        let mut app = App::new(board);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let viewport = Rect::new(0, 0, 80, 24);

        scroll_at(viewport, &mut app, 5, 5, 3, &theme);
        assert_eq!(app.column_scroll(0), 1);
        assert_eq!(app.column_scroll(1), 0);
        scroll_at(viewport, &mut app, 60, 5, 3, &theme);
        assert_eq!(app.column_scroll(0), 1);
        assert_eq!(app.column_scroll(1), 1);
        scroll_at(viewport, &mut app, 5, 23, 3, &theme);
        assert_eq!(app.column_scroll(0), 1);

        app.select_target(0, Some(0));
        app.open_selected_task_details();
        let (modal, _) = task_detail_layout(viewport, &app, 0, &theme);
        scroll_at(viewport, &mut app, 0, 0, 3, &theme);
        assert_eq!(app.task_detail_scroll, 0);
        scroll_at(viewport, &mut app, modal.x + 1, modal.y + 1, 3, &theme);
        assert_eq!(app.task_detail_scroll, 3);
        assert_eq!(app.column_scroll(0), 1);
    }

    #[test]
    fn renders_scrolled_board_and_both_detail_dialogs() {
        let mut board = Board::default();
        for index in 1..9 {
            board.add_column(format!("COLUMN {index}")).unwrap();
        }
        for index in 0..8 {
            board.add_task(8, format!("Task {index}"));
        }
        let mut app = App::new(board);
        app.selected_column = 8;
        app.selected_task = Some(7);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.selected_task = None;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.selected_task = Some(7);
        app.mode = Mode::TaskDetails {
            cursor: app.task_detail_count() - 1,
        };
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
    }

    #[test]
    fn hit_testing_distinguishes_cards_columns_and_footer() {
        let mut board = Board::default();
        board.add_column("DONE".into()).unwrap();
        board.add_task(0, "first".into());
        board.add_task(1, "second".into());
        let app = App::new(board);
        let area = Rect::new(0, 0, 70, 20);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();

        assert_eq!(
            hit_test(area, &app, 2, 3, &theme),
            Some(HitTarget::Task { column: 0, task: 0 })
        );
        assert_eq!(
            hit_test(area, &app, 2, 4, &theme),
            Some(HitTarget::Column(0))
        );
        assert_eq!(
            hit_test(area, &app, 2, 2, &theme),
            Some(HitTarget::Column(0))
        );
        assert_eq!(
            hit_test(area, &app, 36, 0, &theme),
            Some(HitTarget::Column(1))
        );
        assert_eq!(
            hit_test(area, &app, 36, 10, &theme),
            Some(HitTarget::Column(1))
        );
        assert_eq!(hit_test(area, &app, 2, 19, &theme), None);
    }

    #[test]
    fn task_details_expose_click_targets_for_close_and_checklist_items() {
        let mut board = Board::default();
        board.add_task(0, "Clickable task".into());
        board.columns[0].tasks[0]
            .checklist
            .push(ChecklistItem::new("Clickable item".into()));
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 0 };
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let viewport = Rect::new(0, 0, 80, 30);
        let (modal, document) = task_detail_layout(viewport, &app, 0, &theme);
        let close = task_details_close_area(modal);

        assert_eq!(close.right(), modal.right() - 1);
        assert_eq!(
            hit_test(viewport, &app, close.x, close.y, &theme),
            Some(HitTarget::TaskDetailsClose)
        );

        let content = modal_content_area(modal);
        let scroll = task_detail_scroll(&app, &document, 0, usize::from(content.height));
        let item_line = document.checklist_ranges[0].1.start;
        let item_y = content.y + usize_to_u16(item_line - scroll);
        assert_eq!(
            hit_test(viewport, &app, content.x + 4, item_y, &theme),
            Some(HitTarget::ChecklistItem(0))
        );

        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(close.x, close.y)].symbol(), "[");
        assert_eq!(buffer[(close.x + 1, close.y)].symbol(), "×");
        assert_eq!(buffer[(close.x + 2, close.y)].symbol(), "]");
    }

    #[test]
    fn disabling_mouse_hides_controls_and_disables_hit_testing() {
        let mut board = Board::default();
        board.add_task(0, "Keyboard only".into());
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 0 };
        let theme = Theme::from_config_with_mouse(&ThemeConfig::default(), false).unwrap();
        let viewport = Rect::new(0, 0, 80, 24);
        let (modal, _) = task_detail_layout(viewport, &app, 0, &theme);
        let close = task_details_close_area(modal);

        assert_eq!(hit_test(viewport, &app, close.x, close.y, &theme), None);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let screen = buffer_text(terminal.backend().buffer(), 80, 24);
        assert!(!screen.contains("[×]"));
        assert!(!screen.contains("wheel"));
        assert!(!screen.contains("PgUp/PgDn scroll"));
        assert!(screen.contains("^u/^d scroll"));
    }

    #[test]
    fn task_details_grow_to_eighty_percent_then_scroll() {
        let mut board = Board::default();
        board.add_task(0, "Tall task".into());
        for index in 1..=20 {
            board.columns[0].tasks[0]
                .checklist
                .push(ChecklistItem::new(format!("Checklist item {index}")));
        }
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 0 };
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let viewport = Rect::new(0, 0, 100, 40);
        let (modal, document) = task_detail_layout(viewport, &app, 0, &theme);

        assert_eq!(modal.height, 32);
        assert!(document.lines.len() > usize::from(modal_content_area(modal).height));
        let content_height = usize::from(modal_content_area(modal).height);
        let visible_rows = task_detail_visible_rows(document.lines.len(), content_height);
        assert_eq!(visible_rows, content_height - 1);
        let max_scroll = document.lines.len() - visible_rows;

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let [content, more_area, _hint_area] = task_detail_modal_areas(modal);
        assert!(!buffer_row_text(terminal.backend().buffer(), 100, content.y).contains("↑ (more)"));
        assert!(
            buffer_row_text(terminal.backend().buffer(), 100, more_area.y).contains("↓ (more)")
        );
        assert_eq!(
            hit_test(viewport, &app, content.x + 4, content.y, &theme),
            None
        );
        assert_eq!(
            hit_test(viewport, &app, more_area.x + 4, more_area.y, &theme),
            None
        );
        let first_checklist_line = document.checklist_ranges[0].1.start;
        let first_checklist_y = content.y + 1 + usize_to_u16(first_checklist_line);
        assert_eq!(
            hit_test(viewport, &app, content.x + 4, first_checklist_y, &theme),
            Some(HitTarget::ChecklistItem(0))
        );

        scroll_task_details(viewport, &mut app, 5, &theme);
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        assert!(buffer_row_text(terminal.backend().buffer(), 100, content.y).contains("↑ (more)"));
        assert!(
            buffer_row_text(terminal.backend().buffer(), 100, more_area.y).contains("↓ (more)")
        );

        scroll_task_details(viewport, &mut app, 1_000, &theme);
        assert_eq!(app.task_detail_scroll, max_scroll);
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        assert!(buffer_row_text(terminal.backend().buffer(), 100, content.y).contains("↑ (more)"));
        assert!(
            !buffer_row_text(terminal.backend().buffer(), 100, more_area.y).contains("↓ (more)")
        );

        scroll_task_details(viewport, &mut app, -3, &theme);
        assert_eq!(app.task_detail_scroll, max_scroll - 3);
        app.task_detail_scroll = 0;
        scroll_task_details_half_page(viewport, &mut app, true, &theme);
        assert_eq!(app.task_detail_scroll, visible_rows / 2);

        let mut short_board = Board::default();
        short_board.add_task(0, "Short task".into());
        let mut short_app = App::new(short_board);
        short_app.selected_task = Some(0);
        short_app.mode = Mode::TaskDetails { cursor: 0 };
        terminal
            .draw(|frame| draw(frame, &short_app, &theme))
            .unwrap();
        let screen = buffer_text(terminal.backend().buffer(), 100, 40);
        assert!(!screen.contains("↑ (more)"));
        assert!(!screen.contains("↓ (more)"));
        let (short_modal, _) = task_detail_layout(viewport, &short_app, 0, &theme);
        let [_short_content, short_more, short_hint] = task_detail_modal_areas(short_modal);
        for x in short_more.x..short_more.right() {
            assert_eq!(terminal.backend().buffer()[(x, short_more.y)].symbol(), " ");
        }
        assert!(
            buffer_row_text(terminal.backend().buffer(), 100, short_hint.y)
                .contains("Enter/e edit")
        );
    }

    #[test]
    fn task_cards_show_checklist_progress_and_completed_checkbox() {
        let mut board = Board::default();
        board.add_task(0, "Progress task".into());
        board.columns[0].tasks[0].checklist = vec![
            ChecklistItem::new("Done".into()),
            ChecklistItem::new("Pending".into()),
        ];
        board.columns[0].tasks[0].checklist[0].toggle();
        let mut app = App::new(board);
        app.selected_task = Some(0);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        assert!(buffer_text(terminal.backend().buffer(), 40, 12).contains("- [ ] 1 / 2"));

        app.board.columns[0].tasks[0].checklist[1].toggle();
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        assert!(buffer_text(terminal.backend().buffer(), 40, 12).contains("- [x] 2 / 2"));
    }

    #[test]
    fn column_headers_connect_to_lane_borders() {
        let app = App::new(Board::default());
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(buffer[(39, 0)].symbol(), "┐");
        assert_eq!(buffer[(0, 2)].symbol(), "├");
        assert_eq!(buffer[(39, 2)].symbol(), "┤");
        assert_eq!(buffer[(20, 2)].symbol(), "─");
        assert_eq!(buffer[(20, 2)].fg, TUI_ACCENT);
        assert_eq!(buffer[(0, 3)].symbol(), "│");
        assert_eq!(buffer[(39, 3)].symbol(), "│");
        assert_eq!(buffer[(0, 3)].fg, TUI_ACCENT);
        assert_eq!(buffer[(39, 3)].fg, TUI_ACCENT);
        assert_eq!(buffer[(0, 0)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(0, 2)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(1, 1)].symbol(), "▊");
        assert_eq!(buffer[(1, 1)].fg, TUI_ACCENT);
        assert_eq!(buffer[(1, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(2, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(3, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(36, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(37, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(38, 1)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(19, 1)].symbol(), "T");
        assert_eq!(buffer[(19, 1)].fg, TUI_ACCENT);
        for x in 1..39 {
            assert_eq!(buffer[(x, 1)].bg, TUI_BACKGROUND);
        }
    }

    #[test]
    fn selected_task_uses_a_full_height_cursor_bar_without_restyling_content() {
        let mut board = Board::default();
        board.add_task(0, "Selected task".into());
        board.columns[0].tasks[0].due_date = NaiveDate::from_ymd_opt(2026, 7, 17);
        let mut app = App::new(board);
        app.selected_task = Some(0);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 3)].symbol(), "▊");
        assert_eq!(buffer[(1, 4)].symbol(), "▊");
        assert_eq!(buffer[(1, 3)].fg, TUI_ACCENT);
        assert_eq!(buffer[(1, 4)].fg, TUI_ACCENT);
        assert_eq!(buffer[(3, 3)].symbol(), "S");
        assert_eq!(buffer[(3, 3)].fg, Color::White);
        assert_eq!(buffer[(5, 4)].symbol(), "-");
        assert_eq!(buffer[(7, 4)].symbol(), "d");
        assert_eq!(buffer[(7, 4)].fg, Color::DarkGray);
        assert_ne!(buffer[(1, 3)].symbol(), "┌");
        assert_ne!(buffer[(38, 3)].symbol(), "┐");
        assert_eq!(buffer[(20, 2)].symbol(), "─");
        assert_eq!(buffer[(20, 2)].fg, Color::Gray);
        assert_eq!(buffer[(0, 0)].fg, Color::Gray);
        assert_eq!(buffer[(39, 3)].fg, Color::Gray);
        assert_eq!(buffer[(1, 3)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(3, 3)].bg, TUI_BACKGROUND);
        assert_eq!(buffer[(7, 4)].bg, TUI_BACKGROUND);
    }

    #[test]
    fn moving_task_uses_an_accent_outline_instead_of_the_cursor_bar() {
        let mut board = Board::default();
        board.add_task(0, "Moving task".into());
        board.columns[0].tasks[0].due_date = NaiveDate::from_ymd_opt(2026, 7, 17);
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 3)].symbol(), "┌");
        assert_eq!(buffer[(38, 3)].symbol(), "┐");
        assert_eq!(buffer[(1, 6)].symbol(), "└");
        assert_eq!(buffer[(38, 6)].symbol(), "┘");
        assert_eq!(buffer[(1, 3)].fg, TUI_ACCENT);
        assert_eq!(buffer[(38, 6)].fg, TUI_ACCENT);
        assert_eq!(buffer[(2, 4)].symbol(), "M");
        assert_eq!(buffer[(2, 4)].fg, Color::White);
        assert_ne!(buffer[(2, 4)].symbol(), "▊");
    }

    #[test]
    fn task_cards_reserve_one_cell_of_right_padding() {
        let mut board = Board::default();
        board.add_task(0, "x".repeat(36));
        let mut app = App::new(board);
        app.selected_task = Some(0);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(37, 3)].symbol(), "x");
        assert_eq!(buffer[(38, 3)].symbol(), " ");

        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(36, 4)].symbol(), "x");
        assert_eq!(buffer[(37, 4)].symbol(), " ");
        assert_eq!(buffer[(38, 4)].symbol(), "│");
    }

    #[test]
    fn task_detail_metadata_is_aligned_and_selection_has_no_background() {
        let mut board = Board::default();
        board.add_task(0, "Task".into());
        board.columns[0].tasks[0].description =
            "This description wraps onto a continuation line with more words".into();
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 1 };
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(50, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let area = centered(Rect::new(0, 0, 50, 20), 92, 20);
        let inner_x = area.x + 1;
        let screen = buffer_text(terminal.backend().buffer(), 50, 20);
        let rows = screen.lines().collect::<Vec<_>>();
        let description_y = rows
            .iter()
            .position(|row| row.contains("Description  "))
            .unwrap() as u16;
        let description = rows[usize::from(description_y)]
            .chars()
            .skip(usize::from(inner_x))
            .collect::<String>();
        let continuation = rows[usize::from(description_y + 1)]
            .chars()
            .skip(usize::from(inner_x))
            .collect::<String>();
        assert!(description.starts_with("› Description  "));
        let value_column = "› Description  ".chars().count();
        assert!(
            continuation
                .chars()
                .take(value_column)
                .all(|value| value == ' ')
        );
        assert_ne!(continuation.chars().nth(value_column), Some(' '));
        let title = rows
            .iter()
            .find(|row| row.contains("Title"))
            .unwrap()
            .chars()
            .skip(usize::from(inner_x))
            .collect::<String>();
        assert_eq!(title.find("Task"), Some(value_column));

        let buffer = terminal.backend().buffer();
        for y in description_y..=description_y + 1 {
            for x in inner_x..area.right() - 1 {
                assert_eq!(buffer[(x, y)].bg, TUI_BACKGROUND);
            }
        }
        assert_eq!(buffer[(inner_x, description_y)].fg, TUI_ACCENT);
        assert_eq!(
            buffer[(inner_x + value_column as u16, description_y + 1)].fg,
            TUI_ACCENT
        );
    }

    #[test]
    fn checklist_is_a_separate_section_with_item_timestamps() {
        let mut board = Board::default();
        board.add_task(0, "Task".into());
        let now = Utc::now();
        board.columns[0].tasks[0].checklist = vec![ChecklistItem {
            text: "Verify the separate checklist layout".into(),
            completed: true,
            added_at: Some(now - Duration::hours(2)),
            completed_at: Some(now - Duration::hours(1)),
        }];
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 4 };
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 80, 24);
        assert!(screen.contains("Checklist"));
        assert!(screen.contains("› [x] Verify the separate checklist layout"));
        assert!(screen.contains("Added 2 hours ago · Completed 1 hour ago"));
        assert!(screen.find("Checklist").unwrap() < screen.find("History").unwrap());
    }

    #[test]
    fn task_details_render_git_history_as_a_styled_aligned_timeline() {
        let mut board = Board::default();
        board.create_tag("urgent", "#E06C75").unwrap();
        board.add_task(0, "New title".into());
        board.columns[0].tasks[0].tags.push("urgent".into());
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 0 };
        let now = Utc::now();
        app.task_history.insert(
            1,
            vec![
                TaskHistoryEvent {
                    at: now - Duration::hours(2),
                    kind: TaskHistoryKind::Created,
                },
                TaskHistoryEvent {
                    at: now - Duration::minutes(8),
                    kind: TaskHistoryKind::Changed {
                        field: "title".into(),
                        from: "Old title".into(),
                        to: "New title".into(),
                    },
                },
                TaskHistoryEvent {
                    at: now - Duration::minutes(3),
                    kind: TaskHistoryKind::TagAdded("urgent".into()),
                },
            ],
        );
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 100, 40);
        assert!(screen.contains("History"));
        assert!(screen.contains("changed title"));
        assert!(screen.contains("from:"));
        assert!(screen.contains("Old title"));
        assert!(screen.contains("New title"));
        assert!(screen.contains("added tag"));
        assert!(screen.contains("minutes ago"));
        assert!(screen.contains("arrows/hjkl select"));

        let rows = screen.lines().collect::<Vec<_>>();
        let (old_y, old_x) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| row.find("Old title").map(|x| (y, x)))
            .unwrap();
        let (new_y, new_x) = rows
            .iter()
            .enumerate()
            .rev()
            .find_map(|(y, row)| row.find("New title").map(|x| (y, x)))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(old_x as u16, old_y as u16)].fg, Color::Red);
        assert_eq!(buffer[(new_x as u16, new_y as u16)].fg, Color::Green);
        assert_eq!(buffer[(0, 39)].fg, TUI_BACKGROUND);
        assert_eq!(buffer[(0, 39)].bg, TUI_BACKGROUND);
    }

    #[test]
    fn relative_timestamps_use_human_units() {
        let now = Utc::now();
        assert_eq!(
            relative_time(now - Duration::minutes(1), now),
            "1 minute ago"
        );
        assert_eq!(relative_time(now - Duration::hours(3), now), "3 hours ago");
        assert_eq!(relative_time(now - Duration::days(4), now), "4 days ago");
    }

    #[test]
    fn history_splits_event_types_and_content_into_aligned_columns() {
        let mut board = Board::default();
        board.add_task(0, "Task".into());
        let app = App::new(board);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let event = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Added {
                field: "due date".into(),
                value: "2026-08-05".into(),
            },
        };
        assert_eq!(history_event_type(&event), "added due date");
        assert_eq!(
            history_event_type_style(&event, &theme).fg,
            Some(theme.success)
        );
        let lines = history_event_content_lines(&app, &event, 60, &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "2026-08-05");

        let moved = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Moved {
                from_column: "TODO".into(),
                from_position: 3,
                to_column: "DOING".into(),
                to_position: 1,
            },
        };
        assert_eq!(history_event_type(&moved), "moved");
        let lines = history_event_content_lines(&app, &moved, 60, &theme);
        assert_eq!(line_text(&lines[0]), "TODO → DOING");

        let added_tag = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::TagAdded("urgent".into()),
        };
        let removed_tag = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::TagRemoved("urgent".into()),
        };
        assert_eq!(history_event_type(&added_tag), "added tag");
        assert_eq!(history_event_type(&removed_tag), "removed tag");

        let checked = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Changed {
                field: "checklist status for \"Run tests\"".into(),
                from: "incomplete".into(),
                to: "complete".into(),
            },
        };
        assert_eq!(history_event_type(&checked), "checked");
        assert_eq!(
            history_event_type_style(&checked, &theme).fg,
            Some(theme.change)
        );
        let lines = history_event_content_lines(&app, &checked, 60, &theme);
        assert_eq!(line_text(&lines[0]), "Run tests");

        let added_item = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Added {
                field: "checklist item 2".into(),
                value: "Ship it".into(),
            },
        };
        let removed_item = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Removed {
                field: "checklist item 2".into(),
                value: "Ship it".into(),
            },
        };
        assert_eq!(history_event_type(&added_item), "added");
        assert_eq!(history_event_type(&removed_item), "removed");
        assert_eq!(
            history_event_type_style(&removed_item, &theme).fg,
            Some(theme.danger)
        );
        let lines = history_event_content_lines(&app, &added_item, 60, &theme);
        assert_eq!(line_text(&lines[0]), "Ship it");

        let created = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Created,
        };
        assert_eq!(
            history_event_type_style(&created, &theme).fg,
            Some(theme.text)
        );

        let mut app = app;
        app.task_history
            .insert(1, vec![event.clone(), moved.clone()]);
        let groups = task_history_line_groups(&app, &app.board.columns[0].tasks[0], 80, &theme);
        let moved_row = line_text(&groups[0][0]);
        let due_row = line_text(&groups[1][0]);
        assert_eq!(moved_row.find("moved"), due_row.find("added due date"));
        assert_eq!(moved_row.find("TODO"), due_row.find("2026-08-05"));
    }

    #[test]
    fn renders_delete_confirmation_for_a_populated_column() {
        let mut board = Board::default();
        board.add_column("DOING".into()).unwrap();
        board.add_task(1, "Keep this task".into());
        let mut app = App::new(board);
        app.selected_column = 1;
        app.handle_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 80, 20);
        assert!(screen.contains("Confirm deletion"));
        assert!(screen.contains("Delete column \"DOING\"?"));
        assert!(screen.contains("1 task will be moved"));
        assert!(screen.contains("> Cancel <"));
        assert!(screen.contains("Delete"));
        assert!(screen.contains("Enter activate"));
        let title_row = screen
            .lines()
            .find(|row| row.contains("Confirm deletion"))
            .unwrap();
        let title_start = title_row[..title_row.find("Confirm deletion").unwrap()]
            .chars()
            .count();
        assert!(
            (title_start * 2 + "Confirm deletion".len()).abs_diff(80) <= 2,
            "title starts at {title_start}: {title_row:?}"
        );

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        let screen = buffer_text(terminal.backend().buffer(), 80, 20);
        assert!(screen.contains("> Delete <"));
    }

    #[test]
    fn empty_column_confirmation_omits_the_task_migration_message() {
        let mut board = Board::default();
        board.add_column("EMPTY".into()).unwrap();
        let mut app = App::new(board);
        app.selected_column = 1;
        app.handle_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 80, 20);
        assert!(screen.contains("Delete column \"EMPTY\"?"));
        assert!(!screen.contains("tasks will be moved"));
        assert!(!screen.contains("task will be moved"));
    }

    #[test]
    fn help_keeps_two_columns_without_a_center_divider() {
        let mut app = App::new(Board::default());
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let area = centered(Rect::new(0, 0, 90, 24), 90, 24);
        let inner = Block::default().borders(Borders::ALL).inner(area);
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);
        let old_divider_x = columns[0].right() - 1;
        let buffer = terminal.backend().buffer();
        for y in inner.y..inner.bottom() {
            assert_ne!(buffer[(old_divider_x, y)].symbol(), "│");
        }
        let screen = buffer_text(buffer, 90, 24);
        assert!(screen.contains("? / Ctrl-/"));
        assert!(screen.contains("Ctrl-G"));
    }

    #[test]
    fn text_input_help_shows_only_textarea_keymaps() {
        let mut app = App::new(Board::default());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 90, 24);
        assert!(screen.contains("Text input · keymap"));
        assert!(screen.contains("arrows / Ctrl-B/F/P/N"));
        assert!(screen.contains("Ctrl-G"));
        assert!(!screen.contains("MOVE MODE"));
        assert!(!screen.contains("DATE PICKER"));
        assert!(!screen.contains("TAG PICKER"));
    }

    #[test]
    fn long_titles_and_metadata_expand_task_cards() {
        let mut board = Board::default();
        board.add_task(
            0,
            "This deliberately long task title remains entirely visible".into(),
        );
        board.columns[0].tasks[0].tags = vec!["release".into(), "important".into()];
        board.columns[0].tasks[0].due_date = NaiveDate::from_ymd_opt(2026, 7, 17);
        let mut app = App::new(board);
        app.selected_task = Some(0);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(32, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let lane = visible_lanes(Rect::new(0, 0, 32, 19), &app)[0].1;
        let body = lane_regions(lane).1;
        let inner = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .inner(body);
        let cards = visible_cards(inner, &app, 0);
        assert!(cards[0].1.height > 4);
        let buffer = terminal.backend().buffer();
        for y in cards[0].1.y..cards[0].1.bottom() {
            assert_eq!(buffer[(cards[0].1.x, y)].symbol(), "▊");
        }
        let screen = buffer_text(terminal.backend().buffer(), 32, 20);
        assert!(screen.contains("entirely"));
        assert!(screen.contains("visible"));
        assert!(screen.contains("release"));
        assert!(screen.contains("important"));
        assert!(screen.contains("2026-07-17"));
    }

    #[test]
    fn long_input_wraps_and_keeps_the_cursor_tail_visible() {
        let mut app = App::new(Board::default());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        let input = "A very long task title that needs several wrapped rows inside the floating input window and keeps the END visible";
        for character in input.chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 40, 20);
        assert!(screen.contains("several wrapped rows"));
        assert!(screen.contains("END"));
        assert!(screen.contains("visible"));
    }

    #[test]
    fn input_wraps_at_the_content_column_with_right_and_bottom_padding() {
        let mut app = App::new(Board::default());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        for character in "ABCDEFGHIJKLMNOPQRSTU".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(8, 8)].symbol(), ">");
        assert_eq!(buffer[(10, 8)].symbol(), "A");
        assert_eq!(buffer[(10, 9)].symbol(), "U");
        assert_eq!(buffer[(8, 9)].symbol(), " ");
        assert_eq!(buffer[(9, 9)].symbol(), " ");
        assert_eq!(buffer[(30, 8)].symbol(), " ");
        assert_eq!(buffer[(30, 9)].symbol(), " ");
        for x in 8..31 {
            assert_eq!(buffer[(x, 10)].symbol(), " ");
        }
        assert!(buffer_row_text(buffer, 40, 11).contains("Ctrl-G"));
    }

    #[test]
    fn input_editor_renders_cursor_without_a_textbox_background_or_stale_hints() {
        let mut app = App::new(Board::default());
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        for character in "abc".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        let screen = buffer_text(buffer, 100, 20);
        assert!(screen.contains("> abc"));
        assert!(screen.contains("Ctrl-G $EDITOR · Ctrl-/ keymap"));
        assert!(!screen.contains("type to edit"));
        assert!(!screen.contains("enter confirm"));
        assert!(!screen.contains("q cancel"));
        let cursor = (0..20)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .find(|&(x, y)| buffer[(x, y)].bg == theme.accent)
            .expect("input cursor should be visible");
        assert_eq!(buffer[cursor].symbol(), "c");
        assert_eq!(buffer[cursor].fg, theme.background);
        assert_eq!(buffer[cursor].bg, theme.accent);
    }

    #[test]
    fn tags_render_with_colored_backgrounds_and_contrasting_text() {
        let mut board = Board::default();
        board.add_task(0, "Tagged task".into());
        board.create_tag("light", "#FFFFFF").unwrap();
        board.create_tag("dark", "#000000").unwrap();
        board.columns[0].tasks[0].tags = vec!["light".into(), "dark".into()];
        let app = App::new(board);
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(5, 4)].symbol(), "-");
        assert_eq!(buffer[(8, 4)].bg, Color::Rgb(255, 255, 255));
        assert_eq!(buffer[(8, 4)].fg, Color::Black);
        assert_eq!(buffer[(16, 4)].bg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(16, 4)].fg, Color::White);
    }

    #[test]
    fn renders_tag_picker_and_new_tag_color_picker() {
        let mut board = Board::default();
        board.add_task(0, "Tagged task".into());
        board.create_tag("existing", "#61AFEF").unwrap();
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 2 };
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::NewTag(_)));
        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();
    }

    #[test]
    fn tag_picker_scrolls_to_keep_new_tag_visible() {
        let mut board = Board::default();
        board.add_task(0, "Tagged task".into());
        for (index, color) in TAG_COLOR_PALETTE.iter().enumerate().take(10) {
            board
                .create_tag(&format!("long-tag-{index}"), color)
                .unwrap();
        }
        let mut app = App::new(board);
        app.selected_task = Some(0);
        app.mode = Mode::TaskDetails { cursor: 2 };
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        for _ in 0..10 {
            app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        }
        let theme = Theme::from_config(&ThemeConfig::default()).unwrap();
        let backend = TestBackend::new(50, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app, &theme)).unwrap();

        let screen = buffer_text(terminal.backend().buffer(), 50, 15);
        assert!(screen.contains("New Tag"));
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> String {
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn buffer_row_text(buffer: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
