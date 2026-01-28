use std::{
    collections::{HashMap, HashSet},
    fs,
    process::{Command, Stdio},
};

use camino::Utf8Path;
use clap::Subcommand;
use eyre::{Result, WrapErr};
use russh::keys::ssh_key::public::KeyData;
use tokio::runtime::Runtime;
use tracing::{info, warn};

use crate::{
    config::config,
    flake::{Flake, System, resolve_systems_metadata},
    sops,
    sops::{CreationRule, KeyGroup},
    ssh::keyscan,
    state::CliState,
    terminal::TERMINAL,
};

#[derive(Debug, Subcommand)]
pub enum Commands {
    Fetch,
    RefreshSops,
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

fn refresh_sops(cli_state: &mut CliState) -> Result<()> {
    let systems = get_systems(&cli_state.flake)?;
    let mut files = HashMap::<&Utf8Path, HashSet<String>>::new();
    for system in systems {
        for path in &system.metadata().sops_files {
            let flake_path = cli_state.flake.metadata.strip_store_path(path)?;
            let Some(key) = system.state().read().public_key else {
                continue;
            };
            files.entry(flake_path).or_default().insert(sops::public_key_to_age(key));
        }
    }

    let json = serde_json::to_string_pretty(&sops::SopsYaml {
        creation_rules: files
            .iter()
            .map(|(path, keys)| CreationRule {
                path_regex: regex::escape(Utf8Path::new("flake").join(path).as_str()),
                key_groups: vec![KeyGroup {
                    age: config().sops.management_keys.iter().chain(keys.iter()).cloned().collect(),
                }],
            })
            .collect(),
    })?;
    fs::write(".sops.yaml", &json).context("write .sops.yaml")?;

    let mut cmd = Command::new("sops");
    cmd.args(["updatekeys", "-y"])
        .args(
            files
                .keys()
                .map(|p| cli_state.flake.metadata.flake_abs_path(p))
                .collect::<Result<Vec<_>>>()?,
        )
        .stdin(Stdio::null());

    let _writer = TERMINAL.writer();
    cmd.spawn()?.wait()?;

    Ok(())
}

#[allow(clippy::mutable_key_type)]
fn fetch(cli_state: &mut CliState) -> Result<()> {
    let systems = get_systems(&cli_state.flake)?;

    let keys = Runtime::new()?.block_on(keyscan(systems.iter().copied()));
    for (system, key) in keys {
        let KeyData::Ed25519(key) = *key.key_data() else {
            continue;
        };

        let mut state = system.state().write();
        if let Some(prev) = state.public_key
            && prev != key
        {
            warn!(system = system.name(), "key changed")
        } else if state.public_key.is_none() {
            info!(system = system.name(), "fetched key");
            state.public_key = Some(key);
        }
    }

    Ok(())
}

pub(super) fn run(cli: &mut CliState, args: &Args) -> Result<()> {
    match args.command {
        Commands::Fetch => fetch(cli),
        Commands::RefreshSops => refresh_sops(cli),
    }
}
