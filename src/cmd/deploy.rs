use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::{ErrorKind, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    process::Command,
    thread::scope,
    time::Duration,
};

use ansi_to_tui::IntoText;
use camino::Utf8PathBuf;
use clap::Args;
use eyre::{Context, OptionExt, Report, Result};
use flume::{Sender, bounded};
use parking_lot::Mutex;
use ratatui::{
    Frame,
    layout::{
        Constraint::{Fill, Length},
        Layout,
    },
    style::Stylize,
    text::{Text, ToSpan},
};
use rustix::process::{Pid, Signal, kill_process};
use ssh_key::PublicKey;
use tap::Tap;
use tempfile::TempDir;
use tracing::{debug, info};

use crate::{
    cmd::SystemSelector,
    command::TTYChild,
    common::temp_builder,
    config::config,
    flake::System,
    sops,
    state::CliState,
    terminal::{ErrorStyle, TERMINAL, handle_ctrlc, print_error},
};

#[derive(Args, Debug)]
pub struct BuildCmd {
    #[command(flatten)]
    systems: SystemSelector,
}

#[derive(Args, Debug)]
pub struct DeployCmd {
    #[command(flatten)]
    systems: SystemSelector,
    #[arg(short, long)]
    reboot: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum Subcommand {
    Build(BuildCmd),
    Deploy(DeployCmd),
}

#[derive(Default, Debug)]
struct RunnerUpdate {
    text: Text<'static>,
}

enum ProgressUpdate {
    Runner(RunnerUpdate),
    Error(Report),
}

type ProgressTx<'a> = Sender<(&'a str, ProgressUpdate)>;

fn resolve_target_host(target_host: &str) -> Result<Option<IpAddr>> {
    let addrs = format!("{target_host}:22").to_socket_addrs()?;

    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(_) => return Ok(Some(addr.ip())),
            Err(err) if err.kind() == ErrorKind::TimedOut => {}
            Err(err) => return Err(err).context("connect failed"),
        }
    }

    Ok(None)
}

struct Runner<'a> {
    system: System<'a, true>,
    child_pid: Mutex<Option<Pid>>,
    cmd: &'a Subcommand,
}

impl<'a> Runner<'a> {
    fn post_update(&self, progress_tx: &ProgressTx<'a>, text: Text<'static>) {
        progress_tx.send((self.system.name(), ProgressUpdate::Runner(RunnerUpdate { text }))).ok();
    }

    fn run_command(
        &self, progress_tx: &ProgressTx<'a>, description: &str, cmd: Command,
    ) -> Result<()> {
        let child = TTYChild::spawn(cmd, false)?;
        *self.child_pid.lock() = Some(child.pid());

        child
            .wait(|line| {
                self.post_update(
                    progress_tx,
                    line.trim_end().into_text().unwrap_or("<parse error>".into()),
                );
            })
            .map_err(ErrorStyle::action(description))?;

        Ok(())
    }

    fn result_path(&self) -> Utf8PathBuf {
        format!("cache/{}", self.system.name()).into()
    }

    fn write_ssh_config(&self, target_host: &str) -> Result<(TempDir, Utf8PathBuf)> {
        let public_key = self.system.state().read().public_key.ok_or_eyre("public key missing")?;

        let dir = temp_builder().tempdir().context("create temp dir")?;
        let ssh_config_path = dir.path().join("config").try_into()?;
        let mut ssh_config = File::create(&ssh_config_path)?;
        let known_hosts = dir.path().join("hosts");

        if let Some(extra_config) = config().ssh.config_file.as_ref() {
            writeln!(ssh_config, "Include {extra_config}")?;
        }

        writeln!(ssh_config, "Host *")?;
        writeln!(ssh_config, "User root")?;
        writeln!(ssh_config, "IdentityAgent none")?;
        writeln!(ssh_config, "PasswordAuthentication no")?;
        writeln!(ssh_config, "StrictHostKeyChecking yes")?;
        writeln!(ssh_config, "UserKnownHostsFile {}", known_hosts.to_str().unwrap())?;
        writeln!(ssh_config, "ControlMaster auto")?;
        writeln!(ssh_config, "ControlPersist 1m")?;
        writeln!(ssh_config, "ControlPath {}/%h_%p_%r", dir.path().to_str().unwrap())?;

        fs::write(
            known_hosts,
            format!("{target_host} {}", PublicKey::from(public_key).to_string()),
        )?;

        Ok((dir, ssh_config_path))
    }

    fn deploy(&self, progress_tx: &ProgressTx<'a>) -> Result<()> {
        let Some(target_host) = resolve_target_host(&self.system.metadata().target_host)
            .context("resolve target host")?
        else {
            self.post_update(progress_tx, "could not connect".magenta().into());
            return Ok(());
        };
        let target_host = target_host.to_string();

        let (_tmp_dir_guard, ssh_config_file) =
            self.write_ssh_config(&target_host).context("write ssh config")?;

        let toplevel: Utf8PathBuf =
            fs::read_link(self.result_path()).context("read result link")?.try_into()?;

        self.run_command(
            progress_tx,
            "nix copy",
            Command::new("nix").tap_mut(|cmd| {
                cmd.args(["copy", "--no-check-sigs", "--to"])
                    .arg(format!("ssh-ng://{target_host}"))
                    .arg(&toplevel)
                    .env("NIX_SSHOPTS", format!("-F {ssh_config_file}"));
            }),
        )?;

        let ssh_command = || {
            Command::new("ssh").tap_mut(|cmd| {
                cmd.arg("-F").arg(&ssh_config_file).arg(&target_host);
            })
        };

        self.run_command(
            progress_tx,
            "nix-env --set",
            ssh_command().tap_mut(|cmd| {
                cmd.args(["nix-env", "--profile", "/nix/var/nix/profiles/system", "--set"])
                    .arg(&toplevel);
            }),
        )?;

        let &Subcommand::Deploy(DeployCmd { reboot, .. }) = self.cmd else { unreachable!() };

        self.run_command(
            progress_tx,
            "switch-to-configuration",
            ssh_command().tap_mut(|cmd| {
                cmd.args([
                    "systemd-run",
                    "-E",
                    "LOCALE_ARCHIVE",
                    // "-E",
                    // "NIXOS_INSTALL_BOOTLOADER=$installBootloader",
                    "--collect",
                    "--no-ask-password",
                    "--pipe",
                    "--quiet",
                    "--service-type=exec",
                    "--unit=nixos-rebuild-switch-to-configuration",
                    "--wait",
                ])
                .arg(format!("{toplevel}/bin/switch-to-configuration"))
                .arg(if reboot { "boot" } else { "switch" });
            }),
        )?;

        if reboot {
            self.run_command(
                progress_tx,
                "reboot",
                ssh_command().tap_mut(|cmd| {
                    cmd.arg("reboot");
                }),
            )?;
        }

        info!(name = self.system.name(), "deployed");
        self.post_update(progress_tx, "deployed".green().into());

        Ok(())
    }

    fn run(&self, progress_tx: ProgressTx<'a>) -> Result<()> {
        self.run_command(
            &progress_tx,
            "nix build",
            Command::new("nix").tap_mut(|cmd| {
                cmd.arg("build")
                    .arg("--impure")
                    .arg(format!(
                        "./flake#nixosConfigurations.{}.config.system.build.toplevel",
                        self.system.name()
                    ))
                    .arg("-o")
                    .arg(self.result_path());
            }),
        )?;

        if let Subcommand::Deploy(_) = self.cmd {
            debug!(name = self.system.name(), "built");
            self.deploy(&progress_tx)?;
        } else {
            info!(name = self.system.name(), "built");
            self.post_update(&progress_tx, "built".green().into());
        };

        Ok(())
    }

    fn interrupt(&self) {
        if let Some(pid) = &*self.child_pid.lock() {
            kill_process(*pid, Signal::TERM).ok();
        }
    }
}

fn render_progress(frame: &mut Frame, progress: &HashMap<&str, RunnerUpdate>) {
    let mut progress = progress.iter().collect::<Vec<_>>();
    progress.sort_by_key(|it| it.0);

    for ((name, progress), row) in progress.into_iter().zip(frame.area().rows()) {
        let name = name.yellow().bold() + " ".to_span();

        let [name_area, progress_ares] =
            Layout::horizontal([Length(name.width() as u16), Fill(1)]).areas(row);
        frame.render_widget(name, name_area);
        frame.render_widget(&progress.text, progress_ares);
    }
}

pub fn run(state: &mut CliState, cmd: &Subcommand) -> Result<()> {
    let systems = match cmd {
        Subcommand::Build(cmd) => &cmd.systems,
        Subcommand::Deploy(cmd) => &cmd.systems,
    }
    .resolve(&state.flake)
    .context("resolve systems")?;

    for system in &systems {
        for path in &system.metadata().sops_files {
            let path = state.flake.metadata.flake_abs_path(path)?;
            sops::verify_encrypted(&path).context(format!("verify sops file '{path}'"))?;
        }
    }

    let (progress_tx, progress_rx) = bounded(0);

    let runners = systems
        .iter()
        .map(|&system| Runner { system, child_pid: Mutex::new(None), cmd })
        .collect::<Vec<_>>();

    scope(|s| {
        for runner in &runners {
            let progress_tx = progress_tx.clone();
            s.spawn(move || {
                if let Err(err) = runner
                    .run(progress_tx.clone())
                    .map_err(ErrorStyle::system(runner.system.name()))
                {
                    progress_tx.send((runner.system.name(), ProgressUpdate::Error(err))).ok();
                    runner.post_update(&progress_tx, "error".red().into())
                }
            });
        }
        drop(progress_tx);

        let mut progress = HashMap::new();
        let mut interrupted = false;

        TERMINAL.resize(systems.len() as u16);

        while let Ok((system_name, update)) = progress_rx.recv() {
            match update {
                ProgressUpdate::Runner(update) => {
                    progress.insert(system_name, update);
                    TERMINAL.draw(|f| render_progress(f, &progress))?;
                }
                ProgressUpdate::Error(err) => {
                    if interrupted {
                        continue;
                    }

                    print_error(err);
                }
            }

            if handle_ctrlc(Duration::default()) && !interrupted {
                interrupted = true;
                for runner in &runners {
                    runner.interrupt();
                }
            }
        }

        Ok(())
    })
}
