use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{PersistenceConfig, PushMode},
    model::Board,
};

const BOARD_FILE: &str = "board.json";

pub struct Store {
    root: PathBuf,
    push_mode: PushMode,
    push_interval_seconds: u64,
    remote: String,
}

impl Store {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            push_mode: PushMode::Never,
            push_interval_seconds: 300,
            remote: "origin".into(),
        }
    }

    pub fn from_config(config: &PersistenceConfig) -> Self {
        Self {
            root: config.repo.clone(),
            push_mode: config.push,
            push_interval_seconds: config.push_interval_seconds,
            remote: config.remote.clone(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load_or_create(&self) -> Result<Board> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create data directory {}", self.root.display()))?;
        self.ensure_repository()?;

        let path = self.root.join(BOARD_FILE);
        if path.exists() {
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("read board from {}", path.display()))?;
            let mut board: Board = serde_json::from_str(&contents)
                .with_context(|| format!("parse board from {}", path.display()))?;
            board
                .validate_and_repair()
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("validate board from {}", path.display()))?;
            let committed = self.commit_snapshot("Import external board changes")?;
            if committed && self.push_mode == PushMode::EveryChange {
                self.push_now()?;
            }
            Ok(board)
        } else {
            let board = Board::default();
            self.save(&board, "Initialize tdo board")?;
            Ok(board)
        }
    }

    pub fn save(&self, board: &Board, message: &str) -> Result<()> {
        self.ensure_repository()?;
        let path = self.root.join(BOARD_FILE);
        let temporary = self.root.join(".board.json.tmp");
        let mut contents = serde_json::to_string_pretty(board).context("serialize board")?;
        contents.push('\n');
        fs::write(&temporary, contents)
            .with_context(|| format!("write temporary board at {}", temporary.display()))?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("replace board at {}", path.display()))?;
        let committed = self.commit_snapshot(message)?;
        if committed && self.push_mode == PushMode::EveryChange {
            self.push_now()?;
        }
        Ok(())
    }

    /// Pushes in interval mode when the configured interval has elapsed.
    /// TUI callers invoke this on their event-loop tick; CLI callers invoke it
    /// once per command.
    pub fn maybe_push(&self) -> Result<bool> {
        if self.push_mode != PushMode::Interval || !self.push_is_due()? {
            return Ok(false);
        }
        // Record attempts too, so a broken remote does not cause a tight retry loop.
        self.record_push_time()?;
        self.push_now()?;
        Ok(true)
    }

    fn ensure_repository(&self) -> Result<()> {
        if self.root.join(".git").exists() {
            return Ok(());
        }
        let output = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(&self.root)
            .output()
            .context("run git init")?;
        check_git(output, "initialize data repository")
    }

    fn commit_snapshot(&self, message: &str) -> Result<bool> {
        let output = self.git().args(["add", "--", BOARD_FILE]).output()?;
        check_git(output, "stage board state")?;

        let status = self
            .git()
            .args(["diff", "--cached", "--quiet", "--exit-code"])
            .status()
            .context("check staged board state")?;
        if status.success() {
            return Ok(false);
        }
        if status.code() != Some(1) {
            bail!("git could not inspect the staged board state");
        }

        let output = self
            .git()
            .args([
                "-c",
                "user.name=tdo",
                "-c",
                "user.email=tdo@localhost",
                "commit",
                "--quiet",
                "-m",
                message,
            ])
            .output()
            .context("commit board state")?;
        check_git(output, "commit board state")?;
        Ok(true)
    }

    fn push_now(&self) -> Result<()> {
        let output = self
            .git()
            .args(["push", &self.remote, "HEAD"])
            .output()
            .context("push board repository")?;
        check_git(output, "push board repository")?;
        self.record_push_time()
    }

    fn push_is_due(&self) -> Result<bool> {
        let path = self.root.join(".git/tdo-last-push");
        let last_push = match fs::read_to_string(path) {
            Ok(value) => value.trim().parse::<u64>().unwrap_or(0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("read last push time"),
        };
        Ok(unix_time()?.saturating_sub(last_push) >= self.push_interval_seconds)
    }

    fn record_push_time(&self) -> Result<()> {
        fs::write(
            self.root.join(".git/tdo-last-push"),
            unix_time()?.to_string(),
        )
        .context("record last push time")
    }

    fn git(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root);
        command
    }
}

fn unix_time() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

fn check_git(output: Output, operation: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if error.is_empty() {
        bail!("failed to {operation}");
    }
    bail!("failed to {operation}: {error}")
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;

    use crate::config::{PersistenceConfig, PushMode};

    use super::*;

    #[test]
    fn initializes_and_commits_each_changed_snapshot() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path().join("data"));
        let mut board = store.load_or_create().unwrap();
        board.add_task(0, "First task".into());
        store.save(&board, "Add task First task").unwrap();

        let output = Command::new("git")
            .arg("-C")
            .arg(store.root())
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        let log = String::from_utf8(output.stdout).unwrap();
        assert!(log.contains("Initialize tdo board"));
        assert!(log.contains("Add task First task"));
    }

    #[test]
    fn every_change_mode_pushes_each_commit() {
        let directory = tempdir().unwrap();
        let remote = directory.path().join("remote.git");
        let data = directory.path().join("data");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "--quiet"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&data)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&data)
                .args(["remote", "add", "origin"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        let store = Store::from_config(&PersistenceConfig {
            repo: data,
            push: PushMode::EveryChange,
            push_interval_seconds: 300,
            remote: "origin".into(),
        });

        let mut board = store.load_or_create().unwrap();
        board.add_task(0, "Pushed task".into());
        store.save(&board, "Add pushed task").unwrap();

        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["rev-list", "--all", "--count"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "2");
    }
}
