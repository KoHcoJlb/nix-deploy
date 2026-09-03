# Repository Scope

- Ignore `/local` and `/local.just`; they are machine-specific material, not product sources or verification targets.
- Do not inspect `/examples` for architecture or implementation context; it contains sandbox experiments. Repository-wide formatting may still include it.
- The checked-in `justfile` only lists recipes; there is no shared build/test task runner or CI configuration. Use Cargo and Nix commands directly.

# Architecture

- `src/main.rs` only reports top-level failures. CLI startup and command dispatch live in `src/lib.rs` and `src/cmd/`; Nix evaluation/state mapping is in `src/flake.rs`; subprocess/PTY handling is in `src/command.rs` and `src/terminal.rs`.
- `nix/flake.nix` is a library flake: consumers call its `init` output with their own `inputs`, `hosts`, and `values`. It evaluates hosts once for metadata, then rebuilds each system with every host's `deploy.global` definitions merged in.
- `nix/deploy.nix` supplies `deploy.targetHost`, `deploy.tags`, and `deploy.skip`; every host automatically gets its Linux architecture as a tag.

# Runtime Contract

- Run the binary from a deployment workspace, not this source root. The current directory must contain `nix-deploy.toml` and a `flake/` directory; all paths are fixed relative to that directory.
- Every invocation first loads `nix-deploy.toml`, so even `--help` is not context-free. Operational commands then initialize a real stdout TTY, evaluate `path:./flake`, and read/write `systems.json`.
- `eval_store` defaults to `auto`; any other value is canonicalized at startup and must already exist. The flake directory is intentionally evaluated with path syntax so untracked files are visible.
- Builds use `nix build --impure ./flake#nixosConfigurations.<name>.config.system.build.toplevel -o cache/<name>` and run selected systems concurrently.
- System selection requires exactly one of `--names` or `--tags`. Tag selection excludes `deploy.skip` systems and matches when a system contains every requested tag; `--tags all` selects every non-skipped system.

# Deploy And Keys

- Deploys require a previously stored Ed25519 host key in `systems.json`. SSH is generated with `User root`, password auth disabled, the identity agent disabled, and strict host-key checking enabled; `ssh.config_file` is included only as extra config.
- A normal deploy refuses to switch across differing NixOS versions. `--boot` prepares the new generation without switching; `--reboot` prepares it and then reboots.
- Before any build or deploy, every SOPS file reported by the selected systems must already be encrypted; plaintext causes the command to stop before building.
- `keys fetch` scans only systems without a saved key. A changed key is warned about but not accepted automatically.
- `keys refresh-sops` rewrites `.sops.yaml` in the deployment workspace, encrypts new files in place, and runs `sops updatekeys -y` on existing encrypted files. It requires `sops` on `PATH` and can modify secrets.

# Verification

- Fast Rust compile check: `cargo check --lib --bin nix-deploy`.
- Lint the product targets with `cargo clippy --lib --bin nix-deploy`. Do not assume `-D warnings` currently passes; there is a known `clippy::unnecessary_unwrap` warning in `src/terminal.rs`.
- Format Rust with nightly because `rustfmt.toml` uses unstable import options: `cargo +nightly fmt --all -- --check`; omit `-- --check` to apply formatting.
- Run one unit test with `cargo test --lib <test-path>`, but note that the current `line_reader::tests::test_line_reader` test fails because its first read returns `Incomplete`.
- Parse-check changed Nix files with `nix-instantiate --parse <file> >/dev/null`. The repository's `nix/flake.nix` is not a standalone application flake, so generic `nix flake check ./nix` is not the focused validation path.
- Do not use `cargo run` as routine verification: it needs a deployment workspace, external `nix`/`ssh`/`sops` tools depending on the command, network access for operational commands, and a real TTY.

# Rust Conventions

- `clippy.toml` forbids direct `eyre::Report::is` and `eyre::Report::downcast_ref`; use the chain-aware helpers in `src/util.rs`.
