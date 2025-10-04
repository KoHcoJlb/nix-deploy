#![allow(private_interfaces)]

use std::{collections::HashSet, io::Write};

use clap::{Args, Subcommand};
use eyre::{Context, Result};
use russh::keys::PublicKey;

use crate::{
    flake::{Flake, System, resolve_systems_metadata},
    state::CliState,
    terminal::TERMINAL,
};

mod deploy;
mod state;

#[derive(Args, Debug)]
#[group(multiple = false, required = true)]
struct SystemSelector {
    #[arg(short, long, num_args = 1..)]
    tags: Vec<String>,
    #[arg(short, long, num_args = 1..)]
    names: Vec<String>,
}

impl SystemSelector {
    fn resolve<'a>(&self, systems: &'a Flake) -> Result<Vec<System<'a, true>>> {
        if !self.names.is_empty() {
            let systems = self
                .names
                .iter()
                .map(|name| systems.get_system(name))
                .collect::<Result<Vec<_>>>()?;
            return resolve_systems_metadata(&systems).context("resolve metadata");
        }

        let systems =
            resolve_systems_metadata(&systems.get_systems()).context("resolve metadata")?;

        let tags: HashSet<_> = HashSet::from_iter(self.tags.iter().cloned());
        Ok(systems
            .into_iter()
            .filter(|s| !s.metadata().skip)
            .filter(|s| tags.contains("all") || s.metadata().tags.is_superset(&tags))
            .collect())
    }
}

#[derive(Args, Debug)]
struct ListArgs {
    #[command(flatten)]
    selector: SystemSelector,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(flatten)]
    Build(deploy::Subcommand),
    State(state::Args),
    List(ListArgs),
    Tags,
    Test,
}

fn list(state: &mut CliState, args: &ListArgs) -> Result<()> {
    let systems = args.selector.resolve(&state.flake).context("resolve systems")?;

    // info!(?systems);
    // writeln!(&*TERMINAL, "hello")?;
    // writeln!(&*TERMINAL, "world")?;
    writeln!(TERMINAL.writer(), "{systems:#?}")?;

    Ok(())
}

fn tags(state: &mut CliState) -> Result<()> {
    let tags = resolve_systems_metadata(&state.flake.get_systems())?
        .into_iter()
        .flat_map(|s| &s.metadata().tags)
        .collect::<HashSet<_>>();

    dbg!(tags);

    Ok(())
}

fn test(state: &mut CliState) -> Result<()> {
    // let systems = resolve_systems_metadata(&state.flake.get_systems())?;
    // dbg!(systems);

    let system = state.flake.get_system("misc")?;

    let pkey = system.state().read().public_key;
    dbg!(&pkey);
    if let Some(pkey) = pkey {
        println!("{}", PublicKey::from(pkey).to_string());
    }

    Ok(())
}

pub fn run(state: &mut CliState) -> Result<()> {
    match &state.cli.command {
        Command::Build(cmd) => deploy::run(state, cmd),
        Command::State(args) => state::run(state, args),
        Command::List(args) => list(state, args),
        Command::Tags => tags(state),
        Command::Test => test(state),
    }
}
