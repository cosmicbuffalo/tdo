use std::{env, ffi::OsString, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Use a specific configuration file
    #[arg(long, global = true, env = "TDO_CONFIG")]
    pub config: Option<PathBuf>,

    /// Override the configured Git-backed board directory
    #[arg(long, global = true, env = "TDO_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Print command output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the entire board
    List,
    /// Inspect or change columns
    Column(ColumnArgs),
    /// Inspect or change tasks
    Task(TaskArgs),
    /// Change checklist items on a task
    Checklist(ChecklistArgs),
    /// Inspect or manage reusable tags
    Tag(TagArgs),
    /// Inspect configuration
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective configuration
    Show,
    /// Print the configuration file path
    Path,
}

#[derive(Debug, Args)]
pub struct ColumnArgs {
    #[command(subcommand)]
    pub command: ColumnCommand,
}

#[derive(Debug, Subcommand)]
pub enum ColumnCommand {
    /// List columns and their stable IDs
    List,
    /// Show one column
    Show { column_id: u64 },
    /// Add a column
    Add { name: String },
    /// Rename a column
    Rename { column_id: u64, name: String },
    /// Delete a column, moving its tasks to the prior column
    Delete { column_id: u64 },
}

#[derive(Debug, Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// List tasks, optionally in one column
    List {
        #[arg(long)]
        column: Option<u64>,
    },
    /// Show one task
    Show { task_id: u64 },
    /// Show the semantic event history derived from Git commits
    History { task_id: u64 },
    /// Add a task to a column
    Add {
        column_id: u64,
        title: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long, value_name = "YYYY-MM-DD")]
        due: Option<String>,
    },
    /// Edit any supplied fields on a task
    Edit {
        task_id: u64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long = "tag", conflicts_with = "clear_tags")]
        tags: Vec<String>,
        #[arg(long)]
        clear_tags: bool,
        #[arg(long, value_name = "YYYY-MM-DD", conflicts_with = "clear_due")]
        due: Option<String>,
        #[arg(long)]
        clear_due: bool,
    },
    /// Move a task to a column and optional one-based position
    Move {
        task_id: u64,
        #[arg(long)]
        column: u64,
        #[arg(long)]
        position: Option<usize>,
    },
    /// Delete a task
    Delete { task_id: u64 },
}

#[derive(Debug, Args)]
pub struct ChecklistArgs {
    #[command(subcommand)]
    pub command: ChecklistCommand,
}

#[derive(Debug, Subcommand)]
pub enum ChecklistCommand {
    /// Add an item
    Add { task_id: u64, text: String },
    /// Edit an item by one-based position
    Edit {
        task_id: u64,
        item: usize,
        text: String,
    },
    /// Toggle an item's completed state
    Toggle { task_id: u64, item: usize },
    /// Remove an item by one-based position
    Remove { task_id: u64, item: usize },
}

#[derive(Debug, Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub command: TagCommand,
}

#[derive(Debug, Subcommand)]
pub enum TagCommand {
    /// List created tags and colors
    List,
    /// Show one tag
    Show { tag_id: u64 },
    /// Create a reusable tag
    Create {
        name: String,
        #[arg(long, value_name = "#RRGGBB")]
        color: Option<String>,
    },
    /// Change a tag's color
    SetColor {
        tag_id: u64,
        #[arg(value_name = "#RRGGBB")]
        color: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub persistence: PersistenceConfig,
    pub theme: ThemeConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PersistenceConfig {
    pub repo: PathBuf,
    pub push: PushMode,
    pub push_interval_seconds: u64,
    pub remote: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushMode {
    #[default]
    Never,
    EveryChange,
    Interval,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub background: String,
    pub accent: String,
    pub selected_background: String,
    pub border: String,
    pub text: String,
    pub muted: String,
    pub danger: String,
    pub success: String,
}

impl Cli {
    pub fn config_path(&self) -> PathBuf {
        self.config.clone().unwrap_or_else(default_config_path)
    }
}

impl AppConfig {
    pub fn load_or_create(cli: &Cli) -> Result<(Self, PathBuf)> {
        let path = cli.config_path();
        if !path.exists() {
            let config = Self::default();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create config directory {}", parent.display()))?;
            }
            let serialized = toml::to_string_pretty(&config).context("serialize default config")?;
            let contents = format!(
                "# tdo configuration\n# Colors accept names such as orange, cyan, dark_gray, or #RRGGBB.\n# push: never | every_change | interval\n\n{serialized}"
            );
            fs::write(&path, contents)
                .with_context(|| format!("write default config to {}", path.display()))?;
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("read config from {}", path.display()))?;
        let mut config: Self = toml::from_str(&contents)
            .with_context(|| format!("parse config from {}", path.display()))?;
        if let Some(data_dir) = &cli.data_dir {
            config.persistence.repo = data_dir.clone();
        }
        config.persistence.repo = expand_home(&config.persistence.repo);
        config.validate()?;
        Ok((config, path))
    }

    fn validate(&self) -> Result<()> {
        if self.persistence.push_interval_seconds == 0 {
            bail!("persistence.push_interval_seconds must be greater than zero");
        }
        let remote = self.persistence.remote.trim();
        if remote.is_empty() || remote.starts_with('-') {
            bail!("persistence.remote must be a non-empty Git remote name");
        }
        Ok(())
    }
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            repo: default_data_dir(),
            push: PushMode::Never,
            push_interval_seconds: 300,
            remote: "origin".into(),
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "black".into(),
            accent: "orange".into(),
            selected_background: "dark_gray".into(),
            border: "gray".into(),
            text: "white".into(),
            muted: "dark_gray".into(),
            danger: "red".into(),
            success: "green".into(),
        }
    }
}

fn default_config_path() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("tdo/config.toml");
    }
    home_dir().join(".config/tdo/config.toml")
}

fn default_data_dir() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("tdo");
    }
    home_dir().join(".local/share/tdo")
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var_os("HOME").unwrap_or_else(|| OsString::from(".")))
}

fn expand_home(path: &std::path::Path) -> PathBuf {
    if path == std::path::Path::new("~") {
        return home_dir();
    }
    match path.strip_prefix("~/") {
        Ok(remainder) => home_dir().join(remainder),
        Err(_) => path.to_owned(),
    }
}
