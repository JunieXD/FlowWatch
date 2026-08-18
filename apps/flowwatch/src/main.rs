mod alerts;
mod chart;
mod clash_config;
mod cli;
mod collector;
mod commands;
mod dashboard;
mod investigation;
mod paths;
mod update;

use anyhow::Result;
use cli::Cli;

fn main() {
    if let Err(error) = run() {
        eprintln!("错误：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse_localized();
    commands::dispatch(cli)
}
