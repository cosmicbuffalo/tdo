#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    config::{PersistenceConfig, PushMode},
    history::{
        MovementTracking, TaskHistory, TaskHistoryEvent, derive_board_changes_with_movement,
    },
    model::{Board, Column, TagDefinition, Task},
};

const LEGACY_BOARD_FILE: &str = "board.json";
const MANIFEST_FILE: &str = "manifest.json";
const COLUMNS_DIR: &str = "columns";
const TASKS_DIR: &str = "tasks";
const TAGS_DIR: &str = "tags";
const EVENTS_DIR: &str = "events";
const STATE_VERSION: u32 = 2;
const EVENT_VERSION: u32 = 1;
const EVENTS_PER_SEGMENT: u64 = 1_000;
const HISTORY_INDEX_VERSION: i64 = 1;

pub struct Store {
    root: PathBuf,
    push_mode: PushMode,
    push_interval_seconds: u64,
    remote: String,
    last_saved_board: RefCell<Option<Board>>,
    last_seen_revision: RefCell<Option<String>>,
    next_event_id: RefCell<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateMarker {
    modified: SystemTime,
    length: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StateManifest {
    version: u32,
    column_order: Vec<u64>,
    tag_order: Vec<u64>,
    next_column_id: u64,
    next_task_id: u64,
    next_tag_id: u64,
    next_event_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredColumn {
    id: u64,
    title: String,
    task_order: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EventBatch {
    version: u32,
    id: u64,
    recorded_at: DateTime<Utc>,
    message: String,
    events: Vec<LedgerTaskEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LedgerTaskEvent {
    task_id: u64,
    event: TaskHistoryEvent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PendingSave {
    state: StateDelta,
    event_batch: Option<EventBatch>,
    message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StateDelta {
    manifest: StateManifest,
    columns: Vec<StoredColumn>,
    tasks: Vec<Task>,
    tags: Vec<TagDefinition>,
    removed_columns: Vec<u64>,
    removed_tasks: Vec<u64>,
    removed_tags: Vec<u64>,
}

impl Store {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            push_mode: PushMode::Never,
            push_interval_seconds: 300,
            remote: "origin".into(),
            last_saved_board: RefCell::new(None),
            last_seen_revision: RefCell::new(None),
            next_event_id: RefCell::new(1),
        }
    }

    pub fn from_config(config: &PersistenceConfig) -> Self {
        Self {
            root: config.repo.clone(),
            push_mode: config.push,
            push_interval_seconds: config.push_interval_seconds,
            remote: config.remote.clone(),
            last_saved_board: RefCell::new(None),
            last_seen_revision: RefCell::new(None),
            next_event_id: RefCell::new(1),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_marker(&self) -> Result<StateMarker> {
        let metadata = fs::metadata(self.root.join(MANIFEST_FILE))
            .context("inspect persistence manifest for external changes")?;
        Ok(StateMarker {
            modified: metadata
                .modified()
                .context("read persistence manifest modification time")?,
            length: metadata.len(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    pub fn current_revision(&self) -> Result<String> {
        self.current_head()
    }

    pub fn reload_committed_state(&self) -> Result<(String, Board)> {
        let revision = self.current_head()?;
        let (board, manifest) = self.load_v2_revision_state(&revision)?;
        *self.last_saved_board.borrow_mut() = Some(board.clone());
        *self.last_seen_revision.borrow_mut() = Some(revision.clone());
        *self.next_event_id.borrow_mut() = manifest.next_event_id;
        Ok((revision, board))
    }

    pub fn load_or_create(&self) -> Result<Board> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create data directory {}", self.root.display()))?;
        self.ensure_repository()?;
        let _save_lock = self.lock_saves()?;
        self.recover_pending_save()?;

        let board = if self.root.join(MANIFEST_FILE).exists() {
            let (board, mut manifest) = self.load_v2_worktree()?;
            if self.state_is_dirty()? {
                let before = self.load_v2_revision("HEAD").ok();
                if let Some(before) = before {
                    let event_batch = self.board_event_batch(
                        &before,
                        &board,
                        "Import external state changes",
                        &mut manifest,
                    )?;
                    let pending = PendingSave {
                        state: StateDelta::between(Some(&before), &board, manifest.clone()),
                        event_batch,
                        message: "Import external state changes".into(),
                    };
                    self.write_pending_save(&pending)?;
                    self.apply_pending_save(&pending)?;
                } else {
                    let committed = self.commit_snapshot("Import external state changes")?;
                    self.push_after_commit(committed)?;
                }
            }
            *self.next_event_id.borrow_mut() = manifest.next_event_id;
            board
        } else if self.root.join(LEGACY_BOARD_FILE).exists() {
            self.migrate_legacy_board()?
        } else {
            let board = Board::default();
            let manifest = StateManifest::from_board(&board, 1);
            self.write_v2_state(&board, &manifest)?;
            let committed = self.commit_snapshot("Initialize tdo board")?;
            self.push_after_commit(committed)?;
            board
        };

        *self.last_saved_board.borrow_mut() = Some(board.clone());
        *self.last_seen_revision.borrow_mut() = Some(self.current_head()?);
        Ok(board)
    }

    pub fn save(&self, board: &Board, message: &str) -> Result<()> {
        self.ensure_repository()?;
        let _save_lock = self.lock_saves()?;
        let current_revision = self.current_head()?;
        if self
            .last_seen_revision
            .borrow()
            .as_deref()
            .is_some_and(|revision| revision != current_revision.as_str())
        {
            bail!("board changed in another tdo process; reload and retry this edit");
        }
        let mut validated = board.clone();
        validated
            .validate_and_repair()
            .map_err(anyhow::Error::msg)
            .context("validate board before save")?;

        let mut manifest = StateManifest::from_board(&validated, *self.next_event_id.borrow());
        let previous = self.last_saved_board.borrow().clone();
        let event_batch = previous
            .as_ref()
            .map(|previous| self.board_event_batch(previous, &validated, message, &mut manifest))
            .transpose()?
            .flatten();
        let pending = PendingSave {
            state: StateDelta::between(previous.as_ref(), &validated, manifest),
            event_batch,
            message: message.to_owned(),
        };
        self.write_pending_save(&pending)?;
        self.apply_pending_save(&pending)?;
        *self.next_event_id.borrow_mut() = pending.state.manifest.next_event_id;
        *self.last_saved_board.borrow_mut() = Some(validated);
        *self.last_seen_revision.borrow_mut() = Some(self.current_head()?);
        Ok(())
    }

    /// Returns one task's history from a disposable SQLite projection. Git and
    /// the append-only event ledger remain authoritative; the database may be
    /// deleted at any time and will be rebuilt from the current event segments.
    pub fn task_history(&self, task_id: u64) -> Result<Vec<TaskHistoryEvent>> {
        let connection = self.open_history_index()?;
        query_task_history(&connection, task_id, None)
    }

    /// Returns a bounded newest page for interactive rendering plus the number
    /// of older events not loaded into memory.
    pub fn recent_task_history(
        &self,
        task_id: u64,
        limit: usize,
    ) -> Result<(Vec<TaskHistoryEvent>, usize)> {
        let connection = self.open_history_index()?;
        let total: usize = connection.query_row(
            "SELECT COUNT(*) FROM task_events WHERE task_id = ?1",
            [task_id],
            |row| row.get(0),
        )?;
        let history = query_task_history(&connection, task_id, Some(limit))?;
        let earlier = total.saturating_sub(history.len());
        Ok((history, earlier))
    }

    fn open_history_index(&self) -> Result<Connection> {
        let head = self.current_head()?;
        let index_path = self.history_index_path()?;
        if let Some(parent) = index_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create history index directory {}", parent.display()))?;
        }
        let mut connection = Connection::open(&index_path)
            .with_context(|| format!("open history index {}", index_path.display()))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .context("enable WAL mode for history index")?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .context("configure history index durability")?;
        self.prepare_history_index(&mut connection, &head)?;
        Ok(connection)
    }

    fn migrate_legacy_board(&self) -> Result<Board> {
        let path = self.root.join(LEGACY_BOARD_FILE);
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("read legacy board from {}", path.display()))?;
        let mut board: Board = serde_json::from_str(&contents)
            .with_context(|| format!("parse legacy board from {}", path.display()))?;
        board
            .validate_and_repair()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("validate legacy board from {}", path.display()))?;

        // Capture direct edits before walking history so the migration includes
        // every v1 state that Git knows about.
        let imported = self.commit_legacy_snapshot("Import external board changes")?;
        self.push_after_commit(imported)?;
        let snapshots = self.legacy_history_snapshots()?;
        let mut manifest = StateManifest::from_board(&board, 1);
        let mut previous: Option<&Board> = None;
        for (recorded_at, message, snapshot) in &snapshots {
            let history = derive_board_changes_with_movement(
                previous,
                snapshot,
                *recorded_at,
                movement_tracking(message),
            );
            self.append_history_batch(
                history,
                *recorded_at,
                "Imported from legacy board.json history",
                &mut manifest,
            )?;
            previous = Some(snapshot);
        }

        self.write_v2_state(&board, &manifest)?;
        fs::remove_file(&path)
            .with_context(|| format!("remove migrated legacy board {}", path.display()))?;
        let committed = self.commit_snapshot("Migrate persistence to scalable v2 layout")?;
        self.push_after_commit(committed)?;
        *self.next_event_id.borrow_mut() = manifest.next_event_id;
        Ok(board)
    }

    fn board_event_batch(
        &self,
        before: &Board,
        after: &Board,
        message: &str,
        manifest: &mut StateManifest,
    ) -> Result<Option<EventBatch>> {
        let recorded_at = Utc::now();
        let history = derive_board_changes_with_movement(
            Some(before),
            after,
            recorded_at,
            movement_tracking(message),
        );
        self.make_event_batch(history, recorded_at, message, manifest)
    }

    fn append_history_batch(
        &self,
        history: TaskHistory,
        recorded_at: DateTime<Utc>,
        message: &str,
        manifest: &mut StateManifest,
    ) -> Result<()> {
        if let Some(batch) = self.make_event_batch(history, recorded_at, message, manifest)? {
            self.append_event_batch(&batch)?;
        }
        Ok(())
    }

    fn make_event_batch(
        &self,
        history: TaskHistory,
        recorded_at: DateTime<Utc>,
        message: &str,
        manifest: &mut StateManifest,
    ) -> Result<Option<EventBatch>> {
        let events = flatten_history(history);
        if events.is_empty() {
            return Ok(None);
        }
        let batch = EventBatch {
            version: EVENT_VERSION,
            id: manifest.next_event_id,
            recorded_at,
            message: message.to_owned(),
            events,
        };
        manifest.next_event_id = manifest
            .next_event_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("event IDs have been exhausted"))?;
        Ok(Some(batch))
    }

    fn recover_pending_save(&self) -> Result<()> {
        let path = self.pending_save_path()?;
        if !path.exists() {
            return Ok(());
        }
        let pending: PendingSave = read_json(&path).context("read interrupted board save")?;
        self.apply_pending_save(&pending)
            .context("recover interrupted board save")
    }

    fn write_pending_save(&self, pending: &PendingSave) -> Result<()> {
        let path = self.pending_save_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create save journal directory {}", parent.display()))?;
        }
        write_json_atomic(&path, pending).context("write board save journal")
    }

    fn apply_pending_save(&self, pending: &PendingSave) -> Result<()> {
        if let Some(batch) = &pending.event_batch {
            self.append_event_batch(batch)?;
        }
        self.apply_state_delta(&pending.state)?;
        let committed = self.commit_snapshot(&pending.message)?;
        self.push_after_commit(committed)?;
        let path = self.pending_save_path()?;
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("clear board save journal {}", path.display()))?;
        }
        Ok(())
    }

    fn append_event_batch(&self, batch: &EventBatch) -> Result<()> {
        let path = self.root.join(event_segment_path(batch.id));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create event segment directory {}", parent.display()))?;
        }
        let mut batches = if path.exists() {
            read_event_batches(&path)?
        } else {
            Vec::new()
        };
        if let Some(existing) = batches.iter_mut().find(|existing| existing.id == batch.id) {
            *existing = batch.clone();
        } else {
            batches.push(batch.clone());
            batches.sort_by_key(|batch| batch.id);
        }
        write_event_segment_atomic(&path, &batches)
    }

    fn write_v2_state(&self, board: &Board, manifest: &StateManifest) -> Result<()> {
        self.apply_state_delta(&StateDelta::between(None, board, manifest.clone()))
    }

    fn apply_state_delta(&self, delta: &StateDelta) -> Result<()> {
        for directory in [COLUMNS_DIR, TASKS_DIR, TAGS_DIR, EVENTS_DIR] {
            fs::create_dir_all(self.root.join(directory)).with_context(|| {
                format!(
                    "create state directory {}",
                    self.root.join(directory).display()
                )
            })?;
        }
        for column in &delta.columns {
            write_json_atomic(
                &self.root.join(COLUMNS_DIR).join(id_file(column.id)),
                column,
            )?;
        }
        for task in &delta.tasks {
            write_json_atomic(&self.root.join(TASKS_DIR).join(id_file(task.id)), task)?;
        }
        for tag in &delta.tags {
            write_json_atomic(&self.root.join(TAGS_DIR).join(id_file(tag.id)), tag)?;
        }
        for id in &delta.removed_columns {
            remove_if_exists(&self.root.join(COLUMNS_DIR).join(id_file(*id)))?;
        }
        for id in &delta.removed_tasks {
            remove_if_exists(&self.root.join(TASKS_DIR).join(id_file(*id)))?;
        }
        for id in &delta.removed_tags {
            remove_if_exists(&self.root.join(TAGS_DIR).join(id_file(*id)))?;
        }
        // The manifest is the atomic visibility point for counters and ordering.
        write_json_atomic(&self.root.join(MANIFEST_FILE), &delta.manifest)
    }

    fn load_v2_worktree(&self) -> Result<(Board, StateManifest)> {
        let manifest: StateManifest = read_json(&self.root.join(MANIFEST_FILE))?;
        let board = self.assemble_board(&manifest, |path| {
            fs::read(self.root.join(path))
                .with_context(|| format!("read state file {}", self.root.join(path).display()))
        })?;
        Ok((board, manifest))
    }

    fn load_v2_revision(&self, revision: &str) -> Result<Board> {
        self.load_v2_revision_state(revision)
            .map(|(board, _)| board)
    }

    fn load_v2_revision_state(&self, revision: &str) -> Result<(Board, StateManifest)> {
        let manifest_bytes = self.git_file(revision, MANIFEST_FILE)?;
        let manifest: StateManifest =
            serde_json::from_slice(&manifest_bytes).context("parse committed state manifest")?;
        let board = self.assemble_board(&manifest, |path| self.git_file(revision, path))?;
        Ok((board, manifest))
    }

    fn assemble_board<F>(&self, manifest: &StateManifest, mut read: F) -> Result<Board>
    where
        F: FnMut(&str) -> Result<Vec<u8>>,
    {
        if manifest.version != STATE_VERSION {
            bail!("unsupported persistence state version {}", manifest.version);
        }
        let mut tags = Vec::with_capacity(manifest.tag_order.len());
        for id in &manifest.tag_order {
            let path = format!("{TAGS_DIR}/{}", id_file(*id));
            let tag: TagDefinition = serde_json::from_slice(&read(&path)?)
                .with_context(|| format!("parse state file {path}"))?;
            if tag.id != *id {
                bail!("tag file {path} contains id {}", tag.id);
            }
            tags.push(tag);
        }

        let mut seen_tasks = HashSet::new();
        let mut columns = Vec::with_capacity(manifest.column_order.len());
        for id in &manifest.column_order {
            let path = format!("{COLUMNS_DIR}/{}", id_file(*id));
            let stored: StoredColumn = serde_json::from_slice(&read(&path)?)
                .with_context(|| format!("parse state file {path}"))?;
            if stored.id != *id {
                bail!("column file {path} contains id {}", stored.id);
            }
            let mut tasks = Vec::with_capacity(stored.task_order.len());
            for task_id in stored.task_order {
                if !seen_tasks.insert(task_id) {
                    bail!("task id {task_id} appears in more than one column");
                }
                let path = format!("{TASKS_DIR}/{}", id_file(task_id));
                let task: Task = serde_json::from_slice(&read(&path)?)
                    .with_context(|| format!("parse state file {path}"))?;
                if task.id != task_id {
                    bail!("task file {path} contains id {}", task.id);
                }
                tasks.push(task);
            }
            columns.push(Column {
                id: stored.id,
                title: stored.title,
                tasks,
            });
        }

        let mut board = Board {
            version: 1,
            columns,
            tags,
            next_column_id: manifest.next_column_id,
            next_task_id: manifest.next_task_id,
            next_tag_id: manifest.next_tag_id,
        };
        board
            .validate_and_repair()
            .map_err(anyhow::Error::msg)
            .context("validate granular board state")?;
        Ok(board)
    }

    fn prepare_history_index(&self, connection: &mut Connection, head: &str) -> Result<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_events (
                 segment TEXT NOT NULL,
                 batch_id INTEGER NOT NULL,
                 event_index INTEGER NOT NULL,
                 task_id INTEGER NOT NULL,
                 at TEXT NOT NULL,
                 kind_json TEXT NOT NULL,
                 PRIMARY KEY (batch_id, event_index)
             );
             CREATE INDEX IF NOT EXISTS task_events_by_task
                 ON task_events(task_id, batch_id, event_index);",
        )?;
        let indexed_version =
            metadata(connection, "index_version")?.and_then(|value| value.parse::<i64>().ok());
        let indexed_head = metadata(connection, "indexed_head")?;
        if indexed_version == Some(HISTORY_INDEX_VERSION) && indexed_head.as_deref() == Some(head) {
            return Ok(());
        }

        let incremental = indexed_version == Some(HISTORY_INDEX_VERSION)
            && indexed_head
                .as_deref()
                .is_some_and(|old| self.is_ancestor(old, head).unwrap_or(false));
        let segments = if incremental {
            self.changed_event_segments(indexed_head.as_deref().unwrap(), head)?
        } else {
            self.all_event_segments()?
        };

        let transaction = connection.transaction()?;
        if !incremental {
            transaction.execute("DELETE FROM task_events", [])?;
        }
        for segment in segments {
            transaction.execute("DELETE FROM task_events WHERE segment = ?1", [&segment])?;
            let path = self.root.join(&segment);
            if !path.exists() {
                continue;
            }
            for batch in read_event_batches(&path)? {
                if batch.version != EVENT_VERSION {
                    bail!(
                        "event batch {} has unsupported version {}",
                        batch.id,
                        batch.version
                    );
                }
                for (event_index, event) in batch.events.iter().enumerate() {
                    transaction.execute(
                        "INSERT INTO task_events
                         (segment, batch_id, event_index, task_id, at, kind_json)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            segment,
                            batch.id,
                            event_index as u64,
                            event.task_id,
                            event.event.at.to_rfc3339(),
                            serde_json::to_string(&event.event.kind)?,
                        ],
                    )?;
                }
            }
        }
        set_metadata(
            &transaction,
            "index_version",
            &HISTORY_INDEX_VERSION.to_string(),
        )?;
        set_metadata(&transaction, "indexed_head", head)?;
        transaction.commit()?;
        Ok(())
    }

    fn all_event_segments(&self) -> Result<Vec<String>> {
        let directory = self.root.join(EVENTS_DIR);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut segments = fs::read_dir(&directory)
            .with_context(|| format!("read event directory {}", directory.display()))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .map(|entry| format!("{EVENTS_DIR}/{}", entry.file_name().to_string_lossy()))
            .collect::<Vec<_>>();
        segments.sort();
        Ok(segments)
    }

    fn changed_event_segments(&self, old: &str, head: &str) -> Result<Vec<String>> {
        let output = self
            .git()
            .args(["diff", "--name-only", old, head, "--", EVENTS_DIR])
            .output()
            .context("find changed task event segments")?;
        if !output.status.success() {
            return git_error(output, "find changed task event segments");
        }
        let mut segments = String::from_utf8(output.stdout)
            .context("decode changed task event paths")?
            .lines()
            .filter(|path| path.starts_with("events/") && path.ends_with(".jsonl"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        segments.sort();
        segments.dedup();
        Ok(segments)
    }

    fn history_index_path(&self) -> Result<PathBuf> {
        Ok(self.git_dir()?.join("tdo/history-v1.sqlite3"))
    }

    fn pending_save_path(&self) -> Result<PathBuf> {
        Ok(self.git_dir()?.join("tdo/pending-save.json"))
    }

    fn lock_saves(&self) -> Result<fs::File> {
        let path = self.git_dir()?.join("tdo/save.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create save lock directory {}", parent.display()))?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open persistence save lock {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock persistence saves through {}", path.display()))?;
        Ok(file)
    }

    fn git_dir(&self) -> Result<PathBuf> {
        let output = self
            .git()
            .args(["rev-parse", "--git-dir"])
            .output()
            .context("locate persistence Git directory")?;
        if !output.status.success() {
            return git_error(output, "locate persistence Git directory");
        }
        let path = PathBuf::from(
            String::from_utf8(output.stdout)
                .context("decode persistence Git directory")?
                .trim(),
        );
        let git_dir = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        Ok(git_dir)
    }

    fn legacy_history_snapshots(&self) -> Result<Vec<(DateTime<Utc>, String, Board)>> {
        let output = self
            .git()
            .args([
                "log",
                "--reverse",
                "--format=%H%x09%cI%x09%s",
                "--",
                LEGACY_BOARD_FILE,
            ])
            .output()
            .context("read legacy board commit history")?;
        if !output.status.success() {
            return git_error(output, "read legacy board commit history");
        }
        let log = String::from_utf8(output.stdout).context("decode legacy board history")?;
        let mut snapshots = Vec::new();
        for line in log.lines().filter(|line| !line.trim().is_empty()) {
            let mut fields = line.splitn(3, '\t');
            let commit = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("Git omitted a legacy commit ID"))?;
            let timestamp = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("Git omitted a legacy commit timestamp"))?;
            let message = fields.next().unwrap_or_default().to_owned();
            let committed_at = DateTime::parse_from_rfc3339(timestamp)
                .with_context(|| format!("parse commit timestamp {timestamp:?}"))?
                .with_timezone(&Utc);
            let bytes = self.git_file(commit, LEGACY_BOARD_FILE)?;
            let board = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse legacy board snapshot from commit {commit}"))?;
            snapshots.push((committed_at, message, board));
        }
        Ok(snapshots)
    }

    fn git_file(&self, revision: &str, path: &str) -> Result<Vec<u8>> {
        let object = format!("{revision}:{path}");
        let output = self
            .git()
            .args(["show", &object])
            .output()
            .with_context(|| format!("read {path} from revision {revision}"))?;
        if !output.status.success() {
            return git_error(output, &format!("read {path} from revision {revision}"));
        }
        Ok(output.stdout)
    }

    fn state_is_dirty(&self) -> Result<bool> {
        let output = self
            .git()
            .args([
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                MANIFEST_FILE,
                COLUMNS_DIR,
                TASKS_DIR,
                TAGS_DIR,
                EVENTS_DIR,
            ])
            .output()
            .context("inspect external state changes")?;
        if !output.status.success() {
            return git_error(output, "inspect external state changes");
        }
        Ok(!output.stdout.is_empty())
    }

    fn current_head(&self) -> Result<String> {
        let output = self
            .git()
            .args(["rev-parse", "HEAD"])
            .output()
            .context("read persistence repository HEAD")?;
        if !output.status.success() {
            return git_error(output, "read persistence repository HEAD");
        }
        String::from_utf8(output.stdout)
            .context("decode persistence repository HEAD")
            .map(|head| head.trim().to_owned())
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let status = self
            .git()
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .status()
            .context("compare persistence history revisions")?;
        Ok(status.success())
    }

    /// Pushes in interval mode when the configured interval has elapsed.
    /// TUI callers invoke this on their event-loop tick; CLI callers invoke it
    /// once per command.
    pub fn maybe_push(&self) -> Result<bool> {
        if self.push_mode != PushMode::Interval || !self.push_is_due()? {
            return Ok(false);
        }
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

    fn commit_legacy_snapshot(&self, message: &str) -> Result<bool> {
        let output = self.git().args(["add", "--", LEGACY_BOARD_FILE]).output()?;
        check_git(output, "stage legacy board state")?;
        self.commit_staged(message)
    }

    fn commit_snapshot(&self, message: &str) -> Result<bool> {
        let output = self.git().args(["add", "--all", "--", "."]).output()?;
        check_git(output, "stage board state")?;
        self.commit_staged(message)
    }

    fn commit_staged(&self, message: &str) -> Result<bool> {
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

    fn push_after_commit(&self, committed: bool) -> Result<()> {
        if committed && self.push_mode == PushMode::EveryChange {
            self.push_now()?;
        }
        Ok(())
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
        let path = self.git_dir()?.join("tdo-last-push");
        let last_push = match fs::read_to_string(path) {
            Ok(value) => value.trim().parse::<u64>().unwrap_or(0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("read last push time"),
        };
        Ok(unix_time()?.saturating_sub(last_push) >= self.push_interval_seconds)
    }

    fn record_push_time(&self) -> Result<()> {
        fs::write(
            self.git_dir()?.join("tdo-last-push"),
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

impl StateManifest {
    fn from_board(board: &Board, next_event_id: u64) -> Self {
        Self {
            version: STATE_VERSION,
            column_order: board.columns.iter().map(|column| column.id).collect(),
            tag_order: board.tags.iter().map(|tag| tag.id).collect(),
            next_column_id: board.next_column_id,
            next_task_id: board.next_task_id,
            next_tag_id: board.next_tag_id,
            next_event_id,
        }
    }
}

impl StateDelta {
    fn between(before: Option<&Board>, after: &Board, manifest: StateManifest) -> Self {
        let previous_columns = before
            .into_iter()
            .flat_map(|board| &board.columns)
            .map(|column| (column.id, StoredColumn::from(column)))
            .collect::<HashMap<_, _>>();
        let previous_tasks = before
            .into_iter()
            .flat_map(|board| &board.columns)
            .flat_map(|column| &column.tasks)
            .map(|task| (task.id, task))
            .collect::<HashMap<_, _>>();
        let previous_tags = before
            .into_iter()
            .flat_map(|board| &board.tags)
            .map(|tag| (tag.id, tag))
            .collect::<HashMap<_, _>>();

        let current_column_ids = after
            .columns
            .iter()
            .map(|column| column.id)
            .collect::<HashSet<_>>();
        let current_task_ids = after
            .columns
            .iter()
            .flat_map(|column| &column.tasks)
            .map(|task| task.id)
            .collect::<HashSet<_>>();
        let current_tag_ids = after.tags.iter().map(|tag| tag.id).collect::<HashSet<_>>();

        let columns = after
            .columns
            .iter()
            .map(StoredColumn::from)
            .filter(|column| previous_columns.get(&column.id) != Some(column))
            .collect();
        let tasks = after
            .columns
            .iter()
            .flat_map(|column| &column.tasks)
            .filter(|task| previous_tasks.get(&task.id).copied() != Some(*task))
            .cloned()
            .collect();
        let tags = after
            .tags
            .iter()
            .filter(|tag| previous_tags.get(&tag.id).copied() != Some(*tag))
            .cloned()
            .collect();
        let removed_columns = previous_columns
            .keys()
            .filter(|id| !current_column_ids.contains(id))
            .copied()
            .collect();
        let removed_tasks = previous_tasks
            .keys()
            .filter(|id| !current_task_ids.contains(id))
            .copied()
            .collect();
        let removed_tags = previous_tags
            .keys()
            .filter(|id| !current_tag_ids.contains(id))
            .copied()
            .collect();

        Self {
            manifest,
            columns,
            tasks,
            tags,
            removed_columns,
            removed_tasks,
            removed_tags,
        }
    }
}

impl From<&Column> for StoredColumn {
    fn from(column: &Column) -> Self {
        Self {
            id: column.id,
            title: column.title.clone(),
            task_order: column.tasks.iter().map(|task| task.id).collect(),
        }
    }
}

fn flatten_history(history: TaskHistory) -> Vec<LedgerTaskEvent> {
    let mut histories = history.into_iter().collect::<Vec<_>>();
    histories.sort_by_key(|(task_id, _)| *task_id);
    histories
        .into_iter()
        .flat_map(|(task_id, events)| {
            events
                .into_iter()
                .map(move |event| LedgerTaskEvent { task_id, event })
        })
        .collect()
}

fn movement_tracking(message: &str) -> MovementTracking {
    if let Some(task_id) = message
        .strip_prefix("Move task ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
    {
        MovementTracking::Only(task_id)
    } else if message.starts_with("Delete task ") {
        MovementTracking::None
    } else {
        MovementTracking::All
    }
}

fn event_segment_path(event_id: u64) -> String {
    format!("{EVENTS_DIR}/{:016}.jsonl", event_id / EVENTS_PER_SEGMENT)
}

fn id_file(id: u64) -> String {
    format!("{id}.json")
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let mut contents = serde_json::to_vec_pretty(value).context("serialize state file")?;
    contents.push(b'\n');
    fs::write(&temporary, contents)
        .with_context(|| format!("write temporary state file {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace state file {}", path.display()))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read(path).with_context(|| format!("read state file {}", path.display()))?;
    serde_json::from_slice(&contents)
        .with_context(|| format!("parse state file {}", path.display()))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove state file {}", path.display())),
    }
}

fn read_event_batches(path: &Path) -> Result<Vec<EventBatch>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read task event segment {}", path.display()))?;
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).with_context(|| {
                format!("parse {} line {}", path.display(), index.saturating_add(1))
            })
        })
        .collect()
}

fn query_task_history(
    connection: &Connection,
    task_id: u64,
    limit: Option<usize>,
) -> Result<Vec<TaskHistoryEvent>> {
    let order = if limit.is_some() { "DESC" } else { "ASC" };
    let limit_clause = if limit.is_some() { " LIMIT ?2" } else { "" };
    let sql = format!(
        "SELECT at, kind_json FROM task_events
         WHERE task_id = ?1
         ORDER BY batch_id {order}, event_index {order}{limit_clause}"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = match limit {
        Some(limit) => statement.query(params![task_id, limit])?,
        None => statement.query(params![task_id])?,
    };
    let mut history = Vec::new();
    while let Some(row) = rows.next()? {
        let at: String = row.get(0)?;
        let kind: String = row.get(1)?;
        history.push(TaskHistoryEvent {
            at: DateTime::parse_from_rfc3339(&at)
                .with_context(|| format!("parse indexed event timestamp {at:?}"))?
                .with_timezone(&Utc),
            kind: serde_json::from_str(&kind).context("parse indexed task event")?,
        });
    }
    if limit.is_some() {
        history.reverse();
    }
    Ok(history)
}

fn write_event_segment_atomic(path: &Path, batches: &[EventBatch]) -> Result<()> {
    let temporary = path.with_extension("jsonl.tmp");
    let mut contents = Vec::new();
    for batch in batches {
        serde_json::to_writer(&mut contents, batch).context("serialize task event batch")?;
        contents.push(b'\n');
    }
    fs::write(&temporary, contents)
        .with_context(|| format!("write temporary event segment {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("replace event segment {}", path.display()))
}

fn metadata(connection: &Connection, key: &str) -> Result<Option<String>> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(Into::into)
}

fn set_metadata(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO metadata(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
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

fn git_error<T>(output: Output, operation: &str) -> Result<T> {
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

    use crate::{
        config::{PersistenceConfig, PushMode},
        history::TaskHistoryKind,
    };

    use super::*;

    #[test]
    fn initializes_granular_state_and_commits_each_change() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path().join("data"));
        let mut board = store.load_or_create().unwrap();
        board.add_task(0, "First task".into());
        store.save(&board, "Add task First task").unwrap();

        assert!(store.root().join(MANIFEST_FILE).exists());
        assert!(store.root().join("columns/1.json").exists());
        assert!(store.root().join("tasks/1.json").exists());
        assert!(!store.root().join(LEGACY_BOARD_FILE).exists());
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
    fn reloads_committed_changes_from_another_store_instance() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("data");
        let first = Store::new(root.clone());
        let mut first_board = first.load_or_create().unwrap();
        let original_revision = first.current_revision().unwrap();
        let second = Store::new(root);
        let mut second_board = second.load_or_create().unwrap();
        second_board.add_task(0, "Added elsewhere".into());
        second.save(&second_board, "Add task elsewhere").unwrap();
        first_board.add_task(0, "Stale task".into());

        let error = first.save(&first_board, "Stale save").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("board changed in another tdo process")
        );

        let (revision, reloaded) = first.reload_committed_state().unwrap();

        assert_ne!(revision, original_revision);
        assert_ne!(reloaded, first_board);
        assert_eq!(reloaded.columns[0].tasks[0].title, "Added elsewhere");
    }

    #[test]
    fn indexes_history_incrementally_from_event_segments() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path().join("data"));
        let mut board = store.load_or_create().unwrap();
        board.add_task(0, "First title".into());
        store.save(&board, "Add task 1").unwrap();
        assert_eq!(store.task_history(1).unwrap().len(), 1);

        board.columns[0].tasks[0].title = "Second title".into();
        store.save(&board, "Edit task 1").unwrap();
        let history = store.task_history(1).unwrap();
        assert!(history.iter().any(|event| matches!(
            &event.kind,
            TaskHistoryKind::Changed { field, from, to }
                if field == "title" && from == "First title" && to == "Second title"
        )));
        board.columns[0].tasks[0].title = "Third title".into();
        store.save(&board, "Edit task 1 again").unwrap();
        let (recent, earlier) = store.recent_task_history(1, 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(earlier, 1);
        assert!(matches!(
            &recent[1].kind,
            TaskHistoryKind::Changed { to, .. } if to == "Third title"
        ));
        assert!(store.history_index_path().unwrap().exists());
    }

    #[test]
    fn state_delta_writes_only_the_changed_task() {
        let mut before = Board::default();
        before.add_task(0, "Before".into());
        let mut after = before.clone();
        after.columns[0].tasks[0].title = "After".into();
        let delta =
            StateDelta::between(Some(&before), &after, StateManifest::from_board(&after, 2));

        assert!(delta.columns.is_empty());
        assert_eq!(delta.tasks.len(), 1);
        assert_eq!(delta.tasks[0].id, 1);
        assert!(delta.tags.is_empty());
        assert!(delta.removed_tasks.is_empty());
    }

    #[test]
    fn recovers_an_interrupted_save_from_the_git_local_journal() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("data");
        let store = Store::new(root.clone());
        let before = store.load_or_create().unwrap();
        let mut after = before.clone();
        after.add_task(0, "Recovered task".into());
        let mut manifest = StateManifest::from_board(&after, 1);
        let event_batch = store
            .board_event_batch(&before, &after, "Add recovered task", &mut manifest)
            .unwrap();
        let pending = PendingSave {
            state: StateDelta::between(Some(&before), &after, manifest),
            event_batch,
            message: "Add recovered task".into(),
        };
        store.write_pending_save(&pending).unwrap();

        let recovered = Store::new(root);
        let board = recovered.load_or_create().unwrap();
        assert_eq!(board.columns[0].tasks[0].title, "Recovered task");
        assert!(!recovered.pending_save_path().unwrap().exists());
        assert!(matches!(
            recovered.task_history(1).unwrap()[0].kind,
            TaskHistoryKind::Created
        ));
    }

    #[test]
    fn event_segments_roll_over_at_a_bounded_size() {
        assert_eq!(event_segment_path(999), "events/0000000000000000.jsonl");
        assert_eq!(event_segment_path(1_000), "events/0000000000000001.jsonl");
    }

    #[test]
    fn migrates_legacy_board_and_preserves_derived_history() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("data");
        let legacy = Store::new(root.clone());
        fs::create_dir_all(&root).unwrap();
        legacy.ensure_repository().unwrap();
        let mut board = Board::default();
        board.add_task(0, "Before".into());
        fs::write(
            root.join(LEGACY_BOARD_FILE),
            serde_json::to_vec_pretty(&board).unwrap(),
        )
        .unwrap();
        legacy
            .commit_legacy_snapshot("Initialize legacy board")
            .unwrap();
        board.columns[0].tasks[0].title = "After".into();
        fs::write(
            root.join(LEGACY_BOARD_FILE),
            serde_json::to_vec_pretty(&board).unwrap(),
        )
        .unwrap();
        legacy.commit_legacy_snapshot("Edit legacy task").unwrap();

        let store = Store::new(root);
        let migrated = store.load_or_create().unwrap();
        assert_eq!(migrated.columns[0].tasks[0].title, "After");
        assert!(!store.root().join(LEGACY_BOARD_FILE).exists());
        assert!(store.root().join(MANIFEST_FILE).exists());
        assert!(store.task_history(1).unwrap().iter().any(|event| matches!(
            &event.kind,
            TaskHistoryKind::Changed { from, to, .. } if from == "Before" && to == "After"
        )));
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
