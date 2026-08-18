mod chart;
mod clash_config;
mod cli;
mod collector;
mod commands;
mod paths;

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
