mod keys;

use clap::Subcommand;
use eyre::{Context, Result};

use crate::{
    flake::{Flake, System, resolve_systems_metadata},
    state::CliState,
};

#[derive(Debug, Subcommand)]
pub enum Commands {
    Keys(keys::Args),
}

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

fn get_systems(flake: &Flake) -> Result<Vec<System<'_, true>>> {
    let systems = flake.get_systems();
    resolve_systems_metadata(&systems).context("resolve metadata")
}

pub fn run(cli_state: &mut CliState, args: &Args) -> Result<()> {
    match &args.command {
        Commands::Keys(args) => keys::run(cli_state, args),
    }
}
