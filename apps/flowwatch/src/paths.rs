use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use std::path::PathBuf;

pub const AGENT_LABEL: &str = "io.github.flowwatch.agent";

pub struct AppPaths {
    pub database: PathBuf,
    pub uses_default_database: bool,
    pub installed_binary: PathBuf,
    pub command_binary: PathBuf,
    pub launch_agent: PathBuf,
    pub lock_file: PathBuf,
}

impl AppPaths {
    pub fn discover(database_override: Option<PathBuf>) -> Result<Self> {
        let project = ProjectDirs::from("io.github", "FlowWatch", "FlowWatch")
            .context("resolve FlowWatch application directories")?;
        let base = BaseDirs::new().context("resolve user home directory")?;
        let data_dir = project.data_dir().to_path_buf();
        let default_database = data_dir.join("traffic.sqlite3");
        let database = database_override.unwrap_or_else(|| default_database.clone());
        let uses_default_database = database == default_database;
        Ok(Self {
            installed_binary: data_dir.join("bin/flowwatch"),
            command_binary: base.home_dir().join(".local/bin/flowwatch"),
            launch_agent: base
                .home_dir()
                .join("Library/LaunchAgents")
                .join(format!("{AGENT_LABEL}.plist")),
            lock_file: database
                .parent()
                .unwrap_or(&data_dir)
                .join("collector.lock"),
            uses_default_database,
            database,
        })
    }
}
