use std::{
    collections::{HashMap, HashSet},
    fs,
    process::Command,
};

use camino::Utf8Path;
use clap::Subcommand;
use eyre::{Context, Result, eyre};
use ssh_key::{KnownHosts, known_hosts::HostPatterns, public::KeyData};

use crate::{
    command::run_command,
    config::config,
    flake::{Flake, System, resolve_systems_metadata},
    sops,
    sops::{CreationRule, KeyGroup},
    state::CliState,
};

#[derive(Debug, Subcommand)]
pub enum Commands {
    Refresh,
    Sops,
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

fn refresh(cli_state: &mut CliState) -> Result<()> {
    let systems = get_systems(&cli_state.flake)?;

    let mut cmd = Command::new("ssh-keyscan");
    cmd.args(["-t", "ed25519", "-q"]).args(systems.iter().map(|s| &s.metadata().target_host));
    let res = run_command(cmd).context("keyscan")?;

    let hosts = KnownHosts::new(&res);
    for host in hosts {
        let host = host?;

        let (KeyData::Ed25519(key), HostPatterns::Patterns(patterns)) =
            (host.public_key().key_data(), host.host_patterns())
        else {
            continue;
        };

        let host = patterns.first().unwrap();
        systems
            .iter()
            .find(|s| &s.metadata().target_host == host)
            .ok_or(eyre!("system not found for '{host}', probably wrong ssh-keyscan output"))?
            .state()
            .write()
            .public_key = Some(*key)
    }

    Ok(())
}

fn sops(cli_state: &mut CliState) -> Result<()> {
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

pub fn run(cli_state: &mut CliState, args: &Args) -> Result<()> {
    match args.command {
        Commands::Refresh => refresh(cli_state),
        Commands::Sops => sops(cli_state),
    }
}
