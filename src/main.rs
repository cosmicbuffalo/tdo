mod app;
mod commands;
mod config;
mod history;
mod model;
mod storage;
mod ui;

use std::{io, panic, time::Duration};

use anyhow::Result;
use app::{Action, App};
use clap::Parser;
use config::{AppConfig, Cli};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use storage::Store;
use ui::Theme;

const TUI_HISTORY_LIMIT: usize = 200;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, config_path) = AppConfig::load_or_create(&cli)?;
    let store = Store::from_config(&config.persistence);
    let mut board = store.load_or_create()?;

    if cli.command.is_some() {
        return commands::run(&cli, &config, &config_path, &mut board, &store);
    }

    let theme = Theme::from_config(&config.theme)?;
    let mut app = App::new(board);
    app.status = Some(format!("data repo: {}", store.root().display()));
    run_tui(app, store, theme)
}

fn run_tui(mut app: App, store: Store, theme: Theme) -> Result<()> {
    install_panic_restore();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app, &store, &theme);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    store: &Store,
    theme: &Theme,
) -> Result<()> {
    loop {
        load_visible_task_history(app, store);
        terminal.draw(|frame| ui::draw(frame, app, theme))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    let action = app.handle_key(key);
                    match action {
                        Action::None => {}
                        Action::Quit => return Ok(()),
                        Action::Save(message) => match store.save(&app.board, &message) {
                            Ok(()) => {
                                refresh_selected_task_history(app, store);
                                if !app
                                    .status
                                    .as_deref()
                                    .is_some_and(|status| status.starts_with("error:"))
                                {
                                    app.report_saved(&message);
                                }
                            }
                            Err(error) => app.report_error(&error),
                        },
                    }
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    let size = terminal.size()?;
                    let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                    if let Some(target) = ui::hit_test(area, app, mouse.column, mouse.row) {
                        match target {
                            ui::HitTarget::Column(column) => app.select_target(column, None),
                            ui::HitTarget::Task { column, task } => {
                                app.select_target(column, Some(task));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if let Err(error) = store.maybe_push() {
            app.report_error(&error);
        }
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

fn install_panic_restore() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        original_hook(panic_info);
    }));
}
