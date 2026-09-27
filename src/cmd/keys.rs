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
    Fetch {
        #[arg(help = "Fetch keys only for these system names (defaults to all systems)")]
        names: Vec<String>,
        #[arg(short, long, help = "Fetch and replace saved keys, keeping them if fetching fails")]
        reset: bool,
        #[arg(short, long, help = "Refresh sops")]
        sops: bool,
    },
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

    let mut already_encrypted_files = vec![];
    for file in files.keys().map(|p| cli_state.flake.metadata.flake_abs_path(p)) {
        let file = file?;

        if sops::is_encrypted(&file)? {
            already_encrypted_files.push(file);
        } else {
            info!(%file, "encrypt");

            let mut cmd = Command::new("sops");
            cmd.args(["encrypt", "-i"]).arg(file).stdin(Stdio::null());

            let _writer = TERMINAL.writer();
            cmd.spawn()?.wait()?;
        }
    }

    let mut cmd = Command::new("sops");
    cmd.args(["updatekeys", "-y"]).args(already_encrypted_files).stdin(Stdio::null());

    let _writer = TERMINAL.writer();
    cmd.spawn()?.wait()?;

    Ok(())
}

#[allow(clippy::mutable_key_type)]
fn fetch(cli_state: &mut CliState, names: &[String], reset: bool) -> Result<()> {
    let systems = if names.is_empty() {
        cli_state.flake.get_systems()
    } else {
        names.iter().map(|name| cli_state.flake.get_system(name)).collect::<Result<Vec<_>>>()?
    };
    let systems = systems
        .into_iter()
        .filter(|s| reset || s.state().read().public_key.is_none())
        .collect::<Vec<_>>();
    let systems = resolve_systems_metadata(&systems).context("resolve metadata")?;

    let keys = Runtime::new()?.block_on(keyscan(systems));
    for (system, key) in keys {
        let KeyData::Ed25519(key) = *key.key_data() else {
            continue;
        };

        let mut state = system.state().write();
        if let Some(prev) = state.public_key
            && prev != key
            && !reset
        {
            warn!(system = system.name(), "key changed")
        } else if reset || state.public_key.is_none() {
            info!(system = system.name(), "fetched key");
            state.public_key = Some(key);
        }
    }

    Ok(())
}

pub(super) fn run(cli: &mut CliState, args: &Args) -> Result<()> {
    match &args.command {
        Commands::Fetch { names, reset, sops } => {
            fetch(cli, names, *reset)?;
            if *sops {
                refresh_sops(cli)?;
            }
        }
        Commands::RefreshSops => refresh_sops(cli)?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Commands;
    use crate::{Cli, cmd::Command};

    #[test]
    fn fetch_arguments() {
        let cli = Cli::try_parse_from(["nix-deploy", "keys", "fetch"]).unwrap();
        let Command::Keys(args) = cli.command else { panic!("expected keys command") };
        let Commands::Fetch { names, reset, sops } = args.command else {
            panic!("expected fetch command")
        };

        assert!(names.is_empty());
        assert!(!reset);
        assert!(!sops);

        for reset_flag in ["--reset", "-r"] {
            let cli = Cli::try_parse_from([
                "nix-deploy",
                "keys",
                "fetch",
                "host1",
                "host2",
                reset_flag,
                "--sops",
            ])
            .unwrap();
            let Command::Keys(args) = cli.command else { panic!("expected keys command") };
            let Commands::Fetch { names, reset, sops } = args.command else {
                panic!("expected fetch command")
            };

            assert_eq!(names, ["host1", "host2"]);
            assert!(reset);
            assert!(sops);
        }

        assert!(Cli::try_parse_from(["nix-deploy", "keys", "fetch", "--names"]).is_err());
    }
}
