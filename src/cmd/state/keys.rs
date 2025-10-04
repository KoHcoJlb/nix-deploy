use std::{
    collections::{HashMap, HashSet},
    fs,
};

use camino::Utf8Path;
use clap::Subcommand;
use eyre::{Result, WrapErr};
use russh::keys::ssh_key::public::KeyData;
use tokio::runtime::Runtime;
use tracing::{info, warn};

use crate::{
    cmd::state::get_systems,
    config::config,
    sops,
    sops::{CreationRule, KeyGroup},
    ssh::keyscan,
    state::CliState,
};

#[derive(Debug, Subcommand)]
pub enum Commands {
    Fetch,
}

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[allow(clippy::mutable_key_type)]
fn fetch(cli_state: &mut CliState) -> eyre::Result<()> {
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
            .into_iter()
            .map(|(path, keys)| CreationRule {
                path_regex: regex::escape(Utf8Path::new("flake").join(path).as_str()),
                key_groups: vec![KeyGroup {
                    age: config()
                        .sops
                        .management_keys
                        .iter()
                        .cloned()
                        .chain(keys.into_iter())
                        .collect(),
                }],
            })
            .collect(),
    })?;
    fs::write(".sops.yaml", &json).context("write .sops.yaml")?;

    Ok(())
}

pub(super) fn run(cli: &mut CliState, args: &Args) -> Result<()> {
    match args.command {
        Commands::Fetch => fetch(cli),
    }
}
