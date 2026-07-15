use anyhow::{Result, bail};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    app::{App, DeleteChoice, DeleteConfirmation, DeleteTarget, Mode, NewTagField, TagPickerRow},
    config::ThemeConfig,
    history::{TaskHistoryEvent, TaskHistoryKind},
    model::{TAG_COLOR_PALETTE, TagDefinition, Task},
};

const MIN_COLUMN_WIDTH: u16 = 26;
const TASK_CURSOR_WIDTH: u16 = 2;
const TASK_CARD_GAP: u16 = 1;
const TASK_CARD_RIGHT_PADDING: u16 = 1;

pub struct Theme {
    background: Color,
    accent: Color,
    selected_background: Color,
    border: Color,
    text: Color,
    muted: Color,
    danger: Color,
    success: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HitTarget {
    Column(usize),
    Task { column: usize, task: usize },
}

impl Theme {
    pub fn from_config(config: &ThemeConfig) -> Result<Self> {
        Ok(Self {
            background: parse_color(&config.background)?,
            accent: parse_color(&config.accent)?,
            selected_background: parse_color(&config.selected_background)?,
            border: parse_color(&config.border)?,
            text: parse_color(&config.text)?,
            muted: parse_color(&config.muted)?,
            danger: parse_color(&config.danger)?,
            success: parse_color(&config.success)?,
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
        Mode::Help { .. } => draw_help(frame, theme),
        Mode::Board | Mode::Moving(_) => {}
    }
}

pub fn hit_test(area: Rect, app: &App, x: u16, y: u16) -> Option<HitTarget> {
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
    for (task_index, rect) in visible_cards(area, app, column_index) {
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
            "BOARD mode · arrows/hjkl/click navigate · enter details · a add task · C add column · r rename · D delete · m MOVE · ? help · q quit"
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
        Mode::Input(_) => "INPUT mode · type to edit · enter confirm · esc/q cancel · ctrl-u clear",
        Mode::DatePicker(_) => {
            "DATE PICKER mode · arrows/hjkl select · PgUp/PgDn month · enter confirm · d clear · esc/q cancel"
        }
        Mode::TagPicker(_) => {
            "TAG PICKER mode · arrows/hjkl select · enter add/remove/create · esc/q close"
        }
        Mode::NewTag(_) => {
            "NEW TAG mode · type name · tab/arrows choose field · h/l choose color · enter create · esc/q cancel"
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
    let preferred_height = frame.area().height.saturating_sub(4).clamp(20, 34);
    let area = centered(frame.area(), 92, preferred_height);
    let task = app.current_task();
    let content_width = usize::from(area.width.saturating_sub(2)).max(1);
    let tags_index = 2 + task.checklist.len();
    let mut fields = vec![
        ("Title", task.title.clone()),
        (
            "Description",
            if task.description.is_empty() {
                "—".into()
            } else {
                task.description.clone()
            },
        ),
    ];
    for item in &task.checklist {
        fields.push((
            "Checklist",
            format!("[{}] {}", if item.completed { "x" } else { " " }, item.text),
        ));
    }
    fields.push((
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
    ));
    fields.push((
        "Due",
        task.due_date
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "—".into()),
    ));

    // Two border rows and one modal-hint row are reserved by `render_modal`.
    let content_capacity = usize::from(area.height.saturating_sub(3)).max(1);
    let available = (content_capacity / 2)
        .max(5)
        .min(content_capacity.saturating_sub(3).max(1));
    let field_lines = fields
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let selected = index == cursor;
            if index == tags_index && !task.tags.is_empty() {
                task_detail_tag_lines(label, &task.tags, selected, content_width, app, theme)
            } else {
                task_detail_text_lines(label, &value, selected, content_width, theme)
            }
        })
        .collect::<Vec<_>>();
    let start = task_detail_start(&field_lines, cursor, available);
    let mut lines = Vec::new();
    for field in field_lines.into_iter().skip(start) {
        if lines.len() + field.len() > available {
            lines.extend(
                field
                    .into_iter()
                    .take(available.saturating_sub(lines.len())),
            );
            break;
        }
        lines.extend(field);
        if lines.len() == available {
            break;
        }
    }
    if lines.len() < content_capacity {
        lines.push(Line::raw(""));
    }
    if lines.len() < content_capacity {
        lines.push(Line::styled(
            "  History",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ));
    }

    let groups = task_history_line_groups(app, task, content_width, theme);
    let mut hidden_events = app.task_history_earlier.get(&task.id).copied().unwrap_or(0);
    for (index, group) in groups.iter().enumerate() {
        let available = content_capacity.saturating_sub(lines.len());
        if available == 0 {
            hidden_events += groups.len() - index;
            break;
        }
        if group.len() <= available {
            lines.extend(group.iter().cloned());
        } else {
            hidden_events += groups.len() - index;
            break;
        }
    }
    if hidden_events > 0 && lines.len() < content_capacity {
        let message = format!(
            "  … {hidden_events} earlier event{}",
            if hidden_events == 1 { "" } else { "s" }
        );
        lines.push(Line::styled(message, Style::default().fg(theme.muted)));
    }
    render_modal(
        frame,
        area,
        " Task details ",
        lines,
        "hjkl select · Enter/e edit · a add · d delete · Esc/q close · ? help",
        theme,
    );
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
    let prefix_width = 2 + timestamp_width + 2;
    let event_width = width.saturating_sub(prefix_width).max(1);

    events
        .iter()
        .rev()
        .map(|event| {
            let content = history_event_content_lines(app, event, event_width, theme);
            let timestamp = relative_time(event.at, now);
            content
                .into_iter()
                .enumerate()
                .map(|(index, line)| {
                    let mut spans = vec![Span::styled(
                        if index == 0 {
                            format!("  {timestamp:<timestamp_width$}  ")
                        } else {
                            " ".repeat(prefix_width)
                        },
                        Style::default().fg(theme.muted),
                    )];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect()
        })
        .collect()
}

fn history_event_content_lines(
    app: &App,
    event: &TaskHistoryEvent,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.muted);
    match &event.kind {
        TaskHistoryKind::Created => history_text_lines("Created task", width, muted),
        TaskHistoryKind::Moved {
            from_column,
            to_column,
            ..
        } if from_column == to_column => {
            history_text_lines(&format!("Reordered within {from_column}"), width, muted)
        }
        TaskHistoryKind::Moved {
            from_column,
            to_column,
            ..
        } => history_text_lines(
            &format!("Moved from {from_column} to {to_column}"),
            width,
            muted,
        ),
        TaskHistoryKind::Changed { field, from, to } => {
            let mut lines = history_text_lines(&format!("Changed {field} from:"), width, muted);
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
        TaskHistoryKind::Added { field, value } => {
            history_text_lines(&format!("Added {field}: {value}"), width, muted)
        }
        TaskHistoryKind::Removed { field, value } => {
            history_text_lines(&format!("Removed {field}: {value}"), width, muted)
        }
        TaskHistoryKind::TagAdded(tag) => history_tag_event_line(app, "Added tag:", tag, theme),
        TaskHistoryKind::TagRemoved(tag) => history_tag_event_line(app, "Removed tag:", tag, theme),
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

fn history_tag_event_line(app: &App, label: &str, tag: &str, theme: &Theme) -> Vec<Line<'static>> {
    let style = app
        .board
        .tag_by_name(tag)
        .map(tag_style)
        .unwrap_or_else(|| Style::default().fg(theme.text).bg(theme.border));
    vec![Line::from(vec![
        Span::styled(format!("{label} "), Style::default().fg(theme.muted)),
        Span::styled(format!(" {tag} "), style),
    ])]
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
    value: &str,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let style = task_detail_style(selected, theme);
    let prefix = format!("{} {label}: ", if selected { "›" } else { " " });
    let indent = "    ";
    let wrapped = wrap_hanging(
        value,
        width.saturating_sub(Span::raw(&prefix).width()).max(1),
        width.saturating_sub(Span::raw(indent).width()).max(1),
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
                        indent.into()
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
    tags: &[String],
    selected: bool,
    width: usize,
    app: &App,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let label_style = task_detail_style(selected, theme);
    let prefix = format!("{} {label}: ", if selected { "›" } else { " " });
    let indent = "    ";
    let mut lines = Vec::new();
    let mut spans = vec![Span::styled(prefix.clone(), label_style)];
    let mut used = Span::raw(&prefix).width();
    let mut has_tag = false;

    for name in tags {
        let token = format!(" {name} ");
        let token_width = Span::raw(&token).width();
        if has_tag && used + 1 + token_width > width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent, label_style)];
            used = Span::raw(indent).width();
            has_tag = false;
        }
        if has_tag {
            spans.push(Span::raw(" "));
            used += 1;
        }
        if used >= width {
            lines.push(Line::from(spans));
            spans = vec![Span::styled(indent, label_style)];
            used = Span::raw(indent).width();
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
                spans = vec![Span::styled(indent, label_style)];
                used = Span::raw(indent).width();
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

fn task_detail_start(fields: &[Vec<Line<'static>>], cursor: usize, available: usize) -> usize {
    let cursor = cursor.min(fields.len().saturating_sub(1));
    if fields.iter().take(cursor + 1).map(Vec::len).sum::<usize>() <= available {
        return 0;
    }

    let mut start = cursor;
    let mut used = fields.get(cursor).map_or(0, Vec::len).min(available);
    while start > 0 && used + fields[start - 1].len() <= available {
        start -= 1;
        used += fields[start].len();
    }
    start
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
    let mut lines = vec![Line::styled(
        format!("> {}▏", input.text),
        Style::default().fg(theme.text),
    )];
    if let Some(status) = &app.status {
        lines.push(Line::styled(
            status.clone(),
            Style::default().fg(theme.danger),
        ));
    }
    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.selected_background))
        .wrap(Wrap { trim: false });
    let content_height = usize_to_u16(paragraph.line_count(inner_width));
    let height = content_height
        .saturating_add(3)
        .max(6)
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
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let scroll = content_height.saturating_sub(content_area.height);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(paragraph.scroll((scroll, 0)), content_area);
    frame.render_widget(
        Paragraph::new(format!(
            "type to edit · {} · Ctrl-U clear · q cancel · ? help",
            input.kind.hint()
        ))
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
        name: state.name.clone(),
        color: color.into(),
    };
    let mut lines = vec![
        Line::styled(
            format!(
                "{} Name: {}{}",
                if name_selected { "›" } else { " " },
                state.name,
                if name_selected { "▏" } else { "" }
            ),
            if name_selected {
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.selected_background)
            } else {
                Style::default().fg(theme.text)
            },
        ),
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
                format!(
                    " {} ",
                    if state.name.is_empty() {
                        "tag"
                    } else {
                        &state.name
                    }
                ),
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
        "type name · Tab/↑↓ fields · h/l color · Enter continue/create · Esc/q cancel · ? help",
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
        key("q", "close a dialog, or quit from board"),
        key("Ctrl-C", "quit tdo immediately"),
        key("?", "open this keymap"),
        Line::raw(""),
        heading("BOARD"),
        key("arrows / hjkl", "navigate headers and task cards"),
        key("Left click", "select a column or task card"),
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
        key("Enter", "activate the selected field"),
        key("e", "edit the selected field"),
        key("Space", "toggle a checklist item"),
        key("a", "add a checklist item"),
        key("d", "delete a checklist item"),
        key("Esc", "close details"),
        heading("INPUT"),
        key("Enter", "confirm input"),
        key("Esc", "cancel input"),
        key("Ctrl-U", "clear input"),
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
    if let Some(date) = task.due_date {
        lines.extend(task_card_bullet_lines(
            &format!("due {}", date.format("%Y-%m-%d")),
            width,
            Style::default().fg(theme.muted),
        ));
    }
    lines
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
    let tasks = &app.board.columns[column_index].tasks;
    let selected = (column_index == app.selected_column)
        .then_some(app.selected_task)
        .flatten();
    let moving = matches!(app.mode, Mode::Moving(_));
    let start = first_visible_task(tasks, selected, moving, area.width, area.height);
    let mut cards = Vec::new();
    let mut y = area.y;
    for (task_index, task) in tasks.iter().enumerate().skip(start) {
        let remaining = area.bottom().saturating_sub(y);
        if remaining == 0 {
            break;
        }
        let required = task_card_height(task, area.width, moving && selected == Some(task_index));
        if required > remaining && !cards.is_empty() {
            break;
        }
        let height = required.min(remaining);
        cards.push((task_index, Rect::new(area.x, y, area.width, height)));
        y = y
            .saturating_add(height)
            .saturating_add(TASK_CARD_GAP)
            .min(area.bottom());
    }
    cards
}

fn first_visible_task(
    tasks: &[Task],
    selected: Option<usize>,
    moving: bool,
    width: u16,
    available_height: u16,
) -> usize {
    let Some(selected) = selected.filter(|index| *index < tasks.len()) else {
        return 0;
    };
    let height_through_selected: u32 = tasks
        .iter()
        .enumerate()
        .take(selected + 1)
        .map(|(index, task)| {
            u32::from(task_card_footprint(
                task,
                width,
                moving && index == selected,
            ))
        })
        .sum();
    if height_through_selected <= u32::from(available_height) {
        return 0;
    }

    let mut start = selected;
    let mut used = u32::from(task_card_footprint(&tasks[selected], width, moving));
    while start > 0 {
        let previous = u32::from(task_card_footprint(&tasks[start - 1], width, false));
        if used + previous > u32::from(available_height) {
            break;
        }
        used += previous;
        start -= 1;
    }
    start
}

fn task_card_footprint(task: &Task, width: u16, moving: bool) -> u16 {
    task_card_height(task, width, moving).saturating_add(TASK_CARD_GAP)
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

        assert_eq!(
            hit_test(area, &app, 2, 3),
            Some(HitTarget::Task { column: 0, task: 0 })
        );
        assert_eq!(hit_test(area, &app, 2, 4), Some(HitTarget::Column(0)));
        assert_eq!(hit_test(area, &app, 2, 2), Some(HitTarget::Column(0)));
        assert_eq!(hit_test(area, &app, 36, 0), Some(HitTarget::Column(1)));
        assert_eq!(hit_test(area, &app, 36, 10), Some(HitTarget::Column(1)));
        assert_eq!(hit_test(area, &app, 2, 19), None);
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
        assert_eq!(buffer[(20, 2)].fg, Color::Indexed(208));
        assert_eq!(buffer[(0, 3)].symbol(), "│");
        assert_eq!(buffer[(39, 3)].symbol(), "│");
        assert_eq!(buffer[(0, 3)].fg, Color::Indexed(208));
        assert_eq!(buffer[(39, 3)].fg, Color::Indexed(208));
        assert_eq!(buffer[(0, 0)].bg, Color::Black);
        assert_eq!(buffer[(0, 2)].bg, Color::Black);
        assert_eq!(buffer[(1, 1)].symbol(), "▊");
        assert_eq!(buffer[(1, 1)].fg, Color::Indexed(208));
        assert_eq!(buffer[(1, 1)].bg, Color::Black);
        assert_eq!(buffer[(2, 1)].bg, Color::Black);
        assert_eq!(buffer[(3, 1)].bg, Color::Black);
        assert_eq!(buffer[(36, 1)].bg, Color::Black);
        assert_eq!(buffer[(37, 1)].bg, Color::Black);
        assert_eq!(buffer[(38, 1)].bg, Color::Black);
        assert_eq!(buffer[(19, 1)].symbol(), "T");
        assert_eq!(buffer[(19, 1)].fg, Color::Indexed(208));
        for x in 1..39 {
            assert_eq!(buffer[(x, 1)].bg, Color::Black);
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
        assert_eq!(buffer[(1, 3)].fg, Color::Indexed(208));
        assert_eq!(buffer[(1, 4)].fg, Color::Indexed(208));
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
        assert_eq!(buffer[(1, 3)].bg, Color::Black);
        assert_eq!(buffer[(3, 3)].bg, Color::Black);
        assert_eq!(buffer[(7, 4)].bg, Color::Black);
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
        assert_eq!(buffer[(1, 3)].fg, Color::Indexed(208));
        assert_eq!(buffer[(38, 6)].fg, Color::Indexed(208));
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
    fn task_detail_selection_has_no_background_and_continuations_are_indented() {
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
            .position(|row| row.contains("Description:"))
            .unwrap() as u16;
        let description = rows[usize::from(description_y)]
            .chars()
            .skip(usize::from(inner_x))
            .collect::<String>();
        let continuation = rows[usize::from(description_y + 1)]
            .chars()
            .skip(usize::from(inner_x))
            .collect::<String>();
        assert!(description.starts_with("› Description: "));
        assert!(continuation.starts_with("    "));
        assert_ne!(continuation.chars().nth(4), Some(' '));

        let buffer = terminal.backend().buffer();
        for y in description_y..=description_y + 1 {
            for x in inner_x..area.right() - 1 {
                assert_eq!(buffer[(x, y)].bg, Color::Black);
            }
        }
        assert_eq!(buffer[(inner_x, description_y)].fg, Color::Indexed(208));
        assert_eq!(
            buffer[(inner_x + 4, description_y + 1)].fg,
            Color::Indexed(208)
        );
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
        assert!(screen.contains("Changed title from:"));
        assert!(screen.contains("Old title"));
        assert!(screen.contains("New title"));
        assert!(screen.contains("Added tag:"));
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
        assert_eq!(buffer[(0, 39)].fg, Color::Black);
        assert_eq!(buffer[(0, 39)].bg, Color::Black);
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
    fn single_value_history_is_inline_and_moves_show_only_column_names() {
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
        let lines = history_event_content_lines(&app, &event, 60, &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "Added due date: 2026-08-05");

        let moved = TaskHistoryEvent {
            at: Utc::now(),
            kind: TaskHistoryKind::Moved {
                from_column: "TODO".into(),
                from_position: 3,
                to_column: "DOING".into(),
                to_position: 1,
            },
        };
        let lines = history_event_content_lines(&app, &moved, 60, &theme);
        assert_eq!(line_text(&lines[0]), "Moved from TODO to DOING");
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
        assert!(screen.contains("END visible"));
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

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
