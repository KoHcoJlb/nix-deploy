use clap::Parser;
use eyre::{Context, Result};
use flake::Flake;
use tracing_subscriber::fmt;

use crate::{
    cmd::Command,
    state::CliState,
    terminal::{TERMINAL, Uptime, print_error},
};

pub mod cmd;
pub mod command;
pub mod common;
pub mod config;
pub mod flake;
pub mod line_reader;
pub mod sops;
pub mod state;
pub mod terminal;
pub mod util;

#[derive(Parser, Debug)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

pub fn run() -> Result<()> {
    // eyre::set_hook(Box::new(|_| Box::new(EyreHandler))).unwrap();
    config::load_config("nix-deploy.toml").context("load config")?;
    let cli = Cli::parse();

    let _drop_terminal = TERMINAL.init().context("initialize terminal")?;
    fmt()
        .with_env_filter("nix_deploy=trace")
        .with_writer(|| TERMINAL.writer())
        .with_timer(Uptime::default())
        .with_target(false)
        .init();

    let flake = Flake::load().context("load flake")?;
    let mut state = CliState { cli: &cli, flake };

    if let Err(err) = cmd::run(&mut state) {
        print_error(err);
    }

    state.flake.save_system_state().context("save state")?;

    // sleep(Duration::from_secs(11));

    Ok(())
}
