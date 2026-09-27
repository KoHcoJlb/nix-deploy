use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::Write,
    net::{IpAddr, TcpStream, ToSocketAddrs},
    process::Command,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    thread::scope,
    time::Duration,
};

use ansi_to_tui::IntoText;
use camino::Utf8PathBuf;
use clap::Args;
use eyre::{Context, OptionExt, Result};
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
use russh::keys::PublicKey;
use rustix::process::{Pid, Signal, kill_process};
use tap::Tap;
use tempfile::TempDir;
use tracing::{debug, error, info, info_span, warn};

use crate::{
    cmd::SystemSelector,
    command::{TTYChild, run_command},
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
    #[arg(short, long)]
    boot: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum Subcommand {
    Build(BuildCmd),
    Deploy(DeployCmd),
}

#[derive(Default, Debug)]
struct ProgressUpdate {
    text: Text<'static>,
}

type ProgressTx<'a> = Sender<(&'a str, ProgressUpdate)>;

fn resolve_target_host(target_host: &str, target_port: u16) -> Result<IpAddr> {
    let addr = (target_host, target_port).to_socket_addrs()?.next().ok_or_eyre("no address")?;
    TcpStream::connect_timeout(&addr, Duration::from_secs(5)).context("connect failed")?;
    Ok(addr.ip())
}

fn known_hosts_name(target_host: &str, target_port: u16) -> String {
    if target_port == 22 {
        target_host.to_owned()
    } else {
        format!("[{target_host}]:{target_port}")
    }
}

struct Runner<'a> {
    system: System<'a, true>,
    cmd: &'a Subcommand,
    child_pid: Mutex<Option<Pid>>,
    interrupted: AtomicBool,
}

impl<'a> Runner<'a> {
    fn post_update(&self, ptx: &ProgressTx<'a>, text: impl Into<Text<'static>>) {
        ptx.send((self.system.name(), ProgressUpdate { text: text.into() })).ok();
    }

    fn run_tty_command(&self, ptx: &ProgressTx<'a>, description: &str, cmd: Command) -> Result<()> {
        let child = TTYChild::spawn(cmd, false)?;
        *self.child_pid.lock() = Some(child.pid());

        child
            .wait(|line| {
                self.post_update(
                    ptx,
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

        let target_port = self.system.metadata().target_port;
        writeln!(ssh_config, "Port {target_port}")?;

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
            format!(
                "{} {}",
                known_hosts_name(target_host, target_port),
                PublicKey::from(public_key).to_string()
            ),
        )?;

        Ok((dir, ssh_config_path))
    }

    fn deploy(&self, ptx: &ProgressTx<'a>) -> Result<()> {
        let &Subcommand::Deploy(args) = &self.cmd else { unreachable!() };

        let local_toplevel: Utf8PathBuf =
            fs::read_link(self.result_path()).context("read result link")?.try_into()?;
        let local_version = fs::read_to_string(local_toplevel.join("nixos-version"))
            .context("read local version")?;
        debug!(%local_toplevel, %local_version);

        let metadata = self.system.metadata();
        let target_host = match resolve_target_host(&metadata.target_host, metadata.target_port) {
            Ok(host) => host.to_string(),
            Err(err) => {
                error!(?err, "could not connect");
                self.post_update(ptx, "could not connect".magenta());
                return Ok(());
            }
        };

        let (_tmp_dir_guard, ssh_config_file) =
            self.write_ssh_config(&target_host).context("write ssh config")?;

        let ssh_command = || {
            Command::new("ssh").tap_mut(|cmd| {
                cmd.arg("-F").arg(&ssh_config_file).arg(&target_host);
            })
        };

        let remote_toplevel = run_command(ssh_command().tap_mut(|c| {
            c.args(["readlink", "/run/current-system"]);
        }))
        .context("read remote toplevel")?
        .trim()
        .to_owned();
        debug!(remote_toplevel);
        if remote_toplevel == local_toplevel {
            self.post_update(ptx, "already deployed".cyan());
            warn!("already deployed");
            return Ok(());
        }

        let remote_version: String = run_command(ssh_command().tap_mut(|c| {
            c.args(["cat", "/run/current-system/nixos-version"]);
        }))
        .context("read remote version")?
        .trim()
        .to_owned();
        debug!(remote_version);
        if remote_version != local_version && !(args.reboot || args.boot) {
            self.post_update(ptx, "versions differ".red());
            error!("nixos versions differ, refusing to deploy without reboot");
            return Ok(());
        }

        self.run_tty_command(
            ptx,
            "nix copy",
            Command::new("nix").tap_mut(|cmd| {
                cmd.args(["copy", "--no-check-sigs", "--substitute-on-destination", "--to"])
                    .arg(format!("ssh-ng://{target_host}"))
                    .arg(&local_toplevel)
                    .env("NIX_SSHOPTS", format!("-F {ssh_config_file}"));
            }),
        )?;

        self.run_tty_command(
            ptx,
            "nix-env --set",
            ssh_command().tap_mut(|cmd| {
                cmd.args(["nix-env", "--profile", "/nix/var/nix/profiles/system", "--set"])
                    .arg(&local_toplevel);
            }),
        )?;

        self.run_tty_command(
            ptx,
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
                .arg(format!("{local_toplevel}/bin/switch-to-configuration"))
                .arg(if args.boot || args.reboot { "boot" } else { "switch" });
            }),
        )?;

        if args.reboot {
            self.run_tty_command(
                ptx,
                "reboot",
                ssh_command().tap_mut(|cmd| {
                    cmd.arg("reboot");
                }),
            )?;
        }

        info!("deployed");
        self.post_update(ptx, "deployed".green());

        Ok(())
    }

    fn run(&self, ptx: &ProgressTx<'a>) -> Result<()> {
        self.run_tty_command(
            ptx,
            "nix build",
            Command::new("nix").tap_mut(|cmd| {
                cmd.arg("build")
                    .arg("--impure")
                    .arg(format!(
                        "path:./flake#nixosConfigurations.{}.config.system.build.toplevel",
                        self.system.name()
                    ))
                    .arg("-o")
                    .arg(self.result_path());
            }),
        )?;

        if let Subcommand::Deploy(_) = self.cmd {
            debug!("built");
            self.deploy(ptx)?;
        } else {
            info!("built");
            self.post_update(ptx, "built".green());
        };

        Ok(())
    }

    fn interrupt(&self) {
        self.interrupted.store(true, SeqCst);
        if let Some(pid) = *self.child_pid.lock() {
            kill_process(pid, Signal::TERM).ok();
        }
    }
}

fn render_progress(frame: &mut Frame, progress: &HashMap<&str, ProgressUpdate>) {
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
            let path = state
                .flake
                .metadata
                .flake_abs_path(state.flake.metadata.strip_store_path(path)?)?;
            sops::verify_encrypted(&path).context(format!("verify sops file '{path}'"))?;
        }
    }

    let (progress_tx, progress_rx) = bounded(0);
    let runners = systems
        .iter()
        .map(|&system| Runner {
            system,
            cmd,
            child_pid: Mutex::new(None),
            interrupted: Default::default(),
        })
        .collect::<Vec<_>>();

    scope(|s| {
        for runner in &runners {
            let ptx = progress_tx.clone();
            s.spawn(move || {
                let _span = info_span!("", system = runner.system.name()).entered();

                if let Err(err) = runner.run(&ptx)
                    && !runner.interrupted.load(SeqCst)
                {
                    print_error(err);
                    runner.post_update(&ptx, "error".red());
                }
            });
        }
        drop(progress_tx);

        TERMINAL.resize(systems.len() as u16);
        let mut progress = HashMap::new();
        while let Ok((system_name, update)) = progress_rx.recv() {
            progress.insert(system_name, update);
            TERMINAL.draw(|f| render_progress(f, &progress))?;

            if handle_ctrlc(Duration::default()) {
                for runner in &runners {
                    runner.interrupt();
                }
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::{known_hosts_name, resolve_target_host};

    #[test]
    fn target_port_connection_and_host_keys() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let resolved = resolve_target_host("127.0.0.1", addr.port()).unwrap();

        assert_eq!(resolved, addr.ip());
        for host in ["example.test", "127.0.0.1", "::1"] {
            assert_eq!(known_hosts_name(host, 22), host);
            assert_eq!(known_hosts_name(host, 2222), format!("[{host}]:2222"));
        }
    }
}
