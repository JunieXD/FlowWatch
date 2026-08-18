mod clash_config;
mod cli;
mod collector;
mod commands;
mod paths;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    commands::dispatch(cli)
}
