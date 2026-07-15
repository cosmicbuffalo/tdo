mod app;
mod commands;
mod config;
mod history;
mod model;
mod storage;
mod ui;

use std::{
    env, fs,
    io::{self, Write},
    panic,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use app::{Action, App};
use clap::Parser;
use config::{AppConfig, Cli};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use storage::Store;
use ui::Theme;

const TUI_HISTORY_LIMIT: usize = 200;
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskDetailScroll {
    Lines(isize),
    HalfPage(bool),
}

fn task_detail_scroll_command(key: &KeyEvent) -> Option<TaskDetailScroll> {
    match key.code {
        KeyCode::PageUp => Some(TaskDetailScroll::Lines(-5)),
        KeyCode::PageDown => Some(TaskDetailScroll::Lines(5)),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TaskDetailScroll::HalfPage(false))
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TaskDetailScroll::HalfPage(true))
        }
        _ => None,
    }
}

#[derive(Default)]
struct ClickTracker {
    last: Option<(ui::HitTarget, Instant)>,
}

impl ClickTracker {
    fn register(&mut self, target: ui::HitTarget, now: Instant) -> bool {
        let double = self.last.is_some_and(|(previous, at)| {
            previous == target && now.saturating_duration_since(at) <= DOUBLE_CLICK_WINDOW
        });
        self.last = (!double).then_some((target, now));
        double
    }

    fn reset(&mut self) {
        self.last = None;
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, config_path) = AppConfig::load_or_create(&cli)?;
    let store = Store::from_config(&config.persistence);
    let mut board = store.load_or_create()?;

    if cli.command.is_some() {
        return commands::run(&cli, &config, &config_path, &mut board, &store);
    }

    let theme = Theme::from_config_with_mouse(&config.theme, config.input.mouse)?;
    let mut app = App::new(board);
    app.status = Some(format!("data repo: {}", store.root().display()));
    run_tui(app, store, theme, config.input.mouse)
}

fn run_tui(mut app: App, store: Store, theme: Theme, mouse_enabled: bool) -> Result<()> {
    install_panic_restore(mouse_enabled);
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if mouse_enabled {
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout, EnterAlternateScreen)?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app, &store, &theme, mouse_enabled);

    disable_raw_mode()?;
    if mouse_enabled {
        execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
    } else {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }
    terminal.show_cursor()?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    store: &Store,
    theme: &Theme,
    mouse_enabled: bool,
) -> Result<()> {
    let mut known_revision = store.current_revision()?;
    let mut known_state = store.state_marker()?;
    let mut clicks = ClickTracker::default();
    loop {
        reload_external_changes(app, store, &mut known_revision, &mut known_state);
        load_visible_task_history(app, store);
        terminal.draw(|frame| ui::draw(frame, app, theme))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    clicks.reset();
                    let details_scroll = matches!(app.mode, app::Mode::TaskDetails { .. })
                        .then(|| task_detail_scroll_command(&key))
                        .flatten();
                    let action = if let Some(details_scroll) = details_scroll {
                        let size = terminal.size()?;
                        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                        match details_scroll {
                            TaskDetailScroll::HalfPage(down) => {
                                ui::scroll_task_details_half_page(area, app, down, theme)
                            }
                            TaskDetailScroll::Lines(lines) => {
                                ui::scroll_task_details(area, app, lines, theme)
                            }
                        }
                        Action::None
                    } else {
                        app.handle_key(key)
                    };
                    let quit = match action {
                        Action::EditTextExternally => {
                            if let Err(error) =
                                edit_active_text_externally(terminal, app, mouse_enabled)
                            {
                                app.report_error(&error);
                            }
                            false
                        }
                        action => process_action(
                            action,
                            app,
                            store,
                            &mut known_revision,
                            &mut known_state,
                        ),
                    };
                    if quit {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) if mouse_enabled => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let size = terminal.size()?;
                        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                        if let Some(target) =
                            ui::hit_test(area, app, mouse.column, mouse.row, theme)
                        {
                            let action = match target {
                                ui::HitTarget::Column(column) => {
                                    clicks.reset();
                                    app.select_target(column, None);
                                    Action::None
                                }
                                ui::HitTarget::Task { column, task } => {
                                    let double_click = clicks.register(target, Instant::now());
                                    app.select_target(column, Some(task));
                                    if double_click {
                                        app.open_selected_task_details();
                                    }
                                    Action::None
                                }
                                ui::HitTarget::TaskDetailsClose => {
                                    clicks.reset();
                                    app.close_task_details();
                                    Action::None
                                }
                                ui::HitTarget::ChecklistItem(index) => {
                                    clicks.reset();
                                    app.click_checklist_item(index)
                                }
                            };
                            if process_action(
                                action,
                                app,
                                store,
                                &mut known_revision,
                                &mut known_state,
                            ) {
                                return Ok(());
                            }
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        clicks.reset();
                        let size = terminal.size()?;
                        ui::scroll_task_details(
                            ratatui::layout::Rect::new(0, 0, size.width, size.height),
                            app,
                            -3,
                            theme,
                        );
                    }
                    MouseEventKind::ScrollDown => {
                        clicks.reset();
                        let size = terminal.size()?;
                        ui::scroll_task_details(
                            ratatui::layout::Rect::new(0, 0, size.width, size.height),
                            app,
                            3,
                            theme,
                        );
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        if let Err(error) = store.maybe_push() {
            app.report_error(&error);
        }
    }
}

fn edit_active_text_externally(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mouse_enabled: bool,
) -> Result<()> {
    let initial = app
        .active_text_editor_content()
        .context("no active text field to edit")?;
    let editor = env::var("EDITOR").context("$EDITOR is not configured")?;

    suspend_terminal_for_editor(terminal, mouse_enabled)?;
    let edit_result = run_external_editor(&initial, &editor);
    let resume_result = resume_terminal_after_editor(terminal, mouse_enabled);
    resume_result?;
    let edited = edit_result?;
    if !app.replace_active_text_editor_content(&edited) {
        bail!("text field closed while $EDITOR was running");
    }
    Ok(())
}

fn run_external_editor(initial: &str, editor: &str) -> Result<String> {
    let mut command = shlex::split(editor).context("parse $EDITOR")?;
    if command.is_empty() {
        bail!("$EDITOR is empty");
    }
    let mut file = tempfile::Builder::new()
        .prefix("tdo-")
        .suffix(".md")
        .tempfile()
        .context("create temporary editor file")?;
    file.write_all(initial.as_bytes())
        .context("write temporary editor file")?;
    file.flush().context("flush temporary editor file")?;

    let program = command.remove(0);
    let status = Command::new(&program)
        .args(&command)
        .arg(file.path())
        .status()
        .with_context(|| format!("launch $EDITOR ({program})"))?;
    if !status.success() {
        bail!("$EDITOR exited with {status}");
    }
    let mut edited = fs::read_to_string(file.path()).context("read edited text")?;
    if edited.ends_with('\n') {
        edited.pop();
        if edited.ends_with('\r') {
            edited.pop();
        }
    }
    Ok(edited)
}

fn suspend_terminal_for_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse_enabled: bool,
) -> Result<()> {
    disable_raw_mode().context("disable raw mode for $EDITOR")?;
    if mouse_enabled {
        execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        )
        .context("leave tdo screen for $EDITOR")?;
    } else {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)
            .context("leave tdo screen for $EDITOR")?;
    }
    terminal
        .show_cursor()
        .context("show terminal cursor for $EDITOR")?;
    Ok(())
}

fn resume_terminal_after_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse_enabled: bool,
) -> Result<()> {
    enable_raw_mode().context("restore raw mode after $EDITOR")?;
    if mouse_enabled {
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture
        )
        .context("restore tdo screen after $EDITOR")?;
    } else {
        execute!(terminal.backend_mut(), EnterAlternateScreen)
            .context("restore tdo screen after $EDITOR")?;
    }
    terminal.clear().context("redraw tdo after $EDITOR")?;
    Ok(())
}

fn process_action(
    action: Action,
    app: &mut App,
    store: &Store,
    known_revision: &mut String,
    known_state: &mut storage::StateMarker,
) -> bool {
    match action {
        Action::None => false,
        Action::Quit => true,
        Action::EditTextExternally => false,
        Action::Save(message) => {
            match store.save(&app.board, &message) {
                Ok(()) => {
                    match (store.current_revision(), store.state_marker()) {
                        (Ok(revision), Ok(state)) => {
                            *known_revision = revision;
                            *known_state = state;
                        }
                        (Err(error), _) | (_, Err(error)) => {
                            app.report_error(&error);
                            return false;
                        }
                    }
                    refresh_selected_task_history(app, store);
                    if !app
                        .status
                        .as_deref()
                        .is_some_and(|status| status.starts_with("error:"))
                    {
                        app.report_saved(&message);
                    }
                }
                Err(error) => {
                    if let Ok((revision, board)) = store.reload_committed_state() {
                        app.replace_board_from_external_change(board);
                        *known_revision = revision;
                        if let Ok(state) = store.state_marker() {
                            *known_state = state;
                        }
                    }
                    app.report_error(&error);
                }
            }
            false
        }
    }
}

fn reload_external_changes(
    app: &mut App,
    store: &Store,
    known_revision: &mut String,
    known_state: &mut storage::StateMarker,
) {
    if !app.can_reload_external_changes() {
        return;
    }
    let state = match store.state_marker() {
        Ok(state) => state,
        Err(error) => {
            app.report_error(&error);
            return;
        }
    };
    if state == *known_state {
        return;
    }
    let revision = match store.current_revision() {
        Ok(revision) => revision,
        Err(error) => {
            app.report_error(&error);
            return;
        }
    };
    if revision == *known_revision {
        // The manifest is the atomic state-write marker and is written just
        // before Git records the commit. Keep checking until HEAD catches up.
        return;
    }
    match store.reload_committed_state() {
        Ok((revision, board)) => {
            app.replace_board_from_external_change(board);
            *known_revision = revision;
            *known_state = state;
        }
        Err(error) => app.report_error(&error),
    }
}

fn load_visible_task_history(app: &mut App, store: &Store) {
    if !matches!(app.mode, app::Mode::TaskDetails { .. }) {
        return;
    }
    let Some(task_id) = selected_task_id(app) else {
        return;
    };
    if !app.task_history.contains_key(&task_id) {
        refresh_task_history(app, store, task_id);
    }
}

fn refresh_selected_task_history(app: &mut App, store: &Store) {
    if let Some(task_id) = selected_task_id(app) {
        refresh_task_history(app, store, task_id);
    }
}

fn refresh_task_history(app: &mut App, store: &Store, task_id: u64) {
    match store.recent_task_history(task_id, TUI_HISTORY_LIMIT) {
        Ok((history, earlier)) => {
            app.task_history.clear();
            app.task_history_earlier.clear();
            app.task_history.insert(task_id, history);
            if earlier > 0 {
                app.task_history_earlier.insert(task_id, earlier);
            }
        }
        Err(error) => {
            // Mark this task loaded to avoid retrying on every 250 ms draw tick.
            app.task_history.clear();
            app.task_history_earlier.clear();
            app.task_history.entry(task_id).or_default();
            app.report_error(&error.context("load task history"));
        }
    }
}

fn selected_task_id(app: &App) -> Option<u64> {
    app.selected_task.and_then(|task| {
        app.board
            .columns
            .get(app.selected_column)
            .and_then(|column| column.tasks.get(task))
            .map(|task| task.id)
    })
}

fn install_panic_restore(mouse_enabled: bool) {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        if mouse_enabled {
            let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        } else {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
        original_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_tracker_recognizes_only_a_fast_second_click_on_the_same_card() {
        let mut clicks = ClickTracker::default();
        let now = Instant::now();
        let first = ui::HitTarget::Task { column: 0, task: 0 };
        let second = ui::HitTarget::Task { column: 0, task: 1 };

        assert!(!clicks.register(first, now));
        assert!(!clicks.register(second, now + Duration::from_millis(100)));
        assert!(clicks.register(second, now + Duration::from_millis(200)));
        assert!(!clicks.register(first, now + Duration::from_secs(1)));
        assert!(!clicks.register(first, now + Duration::from_secs(2)));
    }

    #[test]
    fn task_details_scroll_with_vim_controls_and_retained_page_keys() {
        assert_eq!(
            task_detail_scroll_command(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(TaskDetailScroll::HalfPage(false))
        );
        assert_eq!(
            task_detail_scroll_command(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(TaskDetailScroll::HalfPage(true))
        );
        assert_eq!(
            task_detail_scroll_command(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(TaskDetailScroll::Lines(-5))
        );
        assert_eq!(
            task_detail_scroll_command(&KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(TaskDetailScroll::Lines(5))
        );
        assert_eq!(
            task_detail_scroll_command(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_editor_round_trips_file_contents_and_trims_file_newline() {
        let edited =
            run_external_editor("before", "sh -c 'printf \"after edit\\n\" > \"$1\"' tdo").unwrap();
        assert_eq!(edited, "after edit");
    }
}
