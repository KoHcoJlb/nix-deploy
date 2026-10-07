# Repository Scope

- Ignore `/local` and `/local.just`; they are machine-specific material, not product sources or verification targets.
- Do not inspect `/examples` for architecture or implementation context; it contains sandbox experiments. Repository-wide formatting may still include it.
- The checked-in `justfile` only lists recipes; there is no shared build/test task runner or CI configuration. Use Cargo and Nix commands directly.

# Architecture

- `src/main.rs` only reports top-level failures. CLI startup and command dispatch live in `src/lib.rs` and `src/cmd/`; Nix evaluation/state mapping is in `src/flake.rs`; subprocess/PTY handling is in `src/command.rs` and `src/terminal.rs`.
- `nix/flake.nix` is a library flake: consumers call `init` with `inputs`, `systems`, `values`, optional `domain` (default `null`), and optional `exportModules`. Inventory entries are Nix system modules: attribute sets, module functions, or file/directory paths (including string paths). Each system has its own lazy `lib.evalModules` evaluation with the schema in `nix/system.nix`. System modules receive normal module arguments (`config`, `options`, `lib`, etc.), `inputs`, `values`, and `system`, an alias for the final system configuration without `_module`. `config` in this layer is system configuration, not NixOS configuration. Evaluated systems are exposed as `init.systems` and passed to NixOS modules as `system`; merged, validated exports are supplied separately as `globalExports`. NixOS configurations retain default unknown-option checking for builds. Metadata uses a separate lazy `extendModules` evaluation with `_module.check = mkForce false`; forcing both paths can evaluate NixOS twice. System-option and export validation remain enabled, and export discovery requires no preliminary NixOS evaluation.
- `exports` and `nixosModule` are `lib.types.deferredModule` options, defaulting to empty modules. Native system-level `imports` compose both layers automatically, preserving option priorities and module source locations. Ordinary NixOS modules belong under `nixosModule`, for example `esphome.nixosModule = ./esphome.nix;`, or inside that module's `imports`. There is no system/module detection, manual system-function invocation, or `parts` mapper. Keep NixOS-dependent code in the deferred NixOS layer.
- `nix/system.nix` declares identity, address, deployment, and deferred-module options. Additional configurable system data must be declared with `options.<name> = lib.mkOption { ... };`; unknown fields fail module validation. Put definitions under `config` when a module also declares `options`. Use option defaults or `mkDefault` for defaults, native `imports` for composition, and `mkForce` for intentional overrides. Keep internal constants local; write fixed service ports directly in settings and exported endpoints instead of adding `port`/`cachePort` bindings and string conversions. Use shared `let` bindings where they add meaning or express derived values. Do not expose implementation details as system options or optional function parameters. There is no `deploy` option namespace or nested facts object.
- `init` supplies `name` from the inventory key and the default domain with `mkDefault`. `fqdn` is derived from the name and effective domain (or just the name when the domain is `null`); `targetHost` defaults to that FQDN. `name`, `fqdn`, and `targetHost'` are read-only options, and redefining them is an error. Read identity/address facts through `system`; do not pass them back unchanged with `inherit (system) domain targetHost`, which would recurse. Inventories declare systems without their own initialization or name injection.
- The `system` alias uses the module system's lazy fixed point, so `domain = "${system.name}.test"` and other `system` references see final option values. Read it in lazy definitions. Imports and option declarations must not depend on final configuration; use `mkIf` for conditional definitions. Defining a field from itself (such as `domain = system.domain`) causes recursion. The inner `nixosModule` has its own NixOS `config`, while `system` continues to refer to the inventory system configuration.
- NixOS `networking.hostName` and `networking.domain` receive `mkDefault` values from the initialized system. NixOS-level overrides affect the NixOS configuration, not the independently declared system facts or exports; change the inventory key or system `domain` to change advertised identity. Templates must not hard-code a deployment domain.
- Deployment overrides are typed system options: `targetHost`, `targetPort`, `tags`, and `skip`. Defaults are the system's FQDN, SSH port 22 (valid range 1–65535), no custom tags, and `skip = false`. Option types are checked when values are forced, independently of NixOS evaluation. Metadata appends the system's Linux architecture tag and discovers SOPS files from that system's configuration. Use `system.targetHost'` for host-and-port/URL rendering; it is derived from the final `targetHost` with `lib.formatHost`, bracketing IPv6 literals without changing hostnames or IPv4 addresses. Deployment metadata uses the raw `targetHost`. Keep DNS aliases and certificate identities based on FQDNs where an IP address would be inappropriate.

# Runtime Contract

- Run the binary from a deployment workspace, not this source root. The current directory must contain `nix-deploy.toml` and a `flake/` directory; all paths are fixed relative to that directory.
- Every invocation first loads `nix-deploy.toml`, so even `--help` is not context-free. Operational commands then initialize a real stdout TTY, evaluate `path:./flake`, and read/write `systems.json`.
- `eval_store` defaults to `auto`; any other value is canonicalized at startup and must already exist. The flake directory is intentionally evaluated with path syntax so untracked files are visible.
- Builds use `nix build --impure path:./flake#nixosConfigurations.<name>.config.system.build.toplevel -o cache/<name>` and run selected systems concurrently.
- `build --jobs N` and `deploy --jobs N` (`-j N`) limit concurrent systems across their entire build/deploy sequence. The limit must be positive; omitting it leaves concurrency unlimited.
- System selection requires exactly one of `--names` or `--tags`. Tag selection excludes systems with `skip = true` and matches when a system contains every requested tag; `--tags all` selects every non-skipped system.

# Deploy And Keys

- Deploys require a previously stored Ed25519 host key in `systems.json`. SSH is generated with `User root`, password auth disabled, the identity agent disabled, and strict host-key checking enabled; `ssh.config_file` is included only as extra config.
- A normal deploy refuses to switch across differing NixOS versions. `--boot` prepares the new generation without switching; `--reboot` prepares it and then reboots.
- Before any build or deploy, every SOPS file reported by the selected systems must already be encrypted; plaintext causes the command to stop before building.
- `keys fetch` scans only systems without a saved key. A changed key is warned about but not accepted automatically.
- `keys refresh-sops` rewrites `.sops.yaml` in the deployment workspace, encrypts new files in place, and runs `sops updatekeys -y` on existing encrypted files. It requires `sops` on `PATH` and can modify secrets.

# Verification

- Fast Rust compile check: `cargo check --lib --bin nix-deploy`.
- Lint the product targets with `cargo clippy --lib --bin nix-deploy`.
- Format Rust with nightly because `rustfmt.toml` uses unstable import options: `cargo +nightly fmt --all -- --check`; omit `-- --check` to apply formatting.
- Run one unit test with `cargo test --lib <test-path>`.
- Parse-check changed Nix files with `nix-instantiate --parse <file> >/dev/null`. The repository's `nix/flake.nix` is not a standalone application flake, so generic `nix flake check ./nix` is not the focused validation path.
- Do not use `cargo run` as routine verification: it needs a deployment workspace, external `nix`/`ssh`/`sops` tools depending on the command, network access for operational commands, and a real TTY.

# Nix Conventions

- When changing Nix files, combine definitions sharing an attribute-set prefix into a single block, such as `options = { ... };` or `networking = { ... };`, rather than repeating that prefix across separate assignments.

# Rust Conventions

- `clippy.toml` forbids direct `eyre::Report::is` and `eyre::Report::downcast_ref`; use the chain-aware helpers in `src/util.rs`.
