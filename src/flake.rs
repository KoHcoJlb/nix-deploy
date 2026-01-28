#![allow(non_upper_case_globals)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    process::Command,
    thread::scope,
};

use Utf8Component::RootDir;
use bstr::ByteSlice;
use by_address::ByAddress;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use eyre::{Context, Result, eyre};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use russh::keys::ssh_key::public::Ed25519PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs, formats, hex::Hex, serde_as};
use tap::Tap;
use tracing::trace;

use crate::{
    command::{nix_eval, run_command_tty},
    config::config,
    terminal::ErrorStyle,
};

const STATE_FILE: &str = "systems.json";

struct PublicKeyFormat;

impl SerializeAs<Ed25519PublicKey> for PublicKeyFormat {
    fn serialize_as<S>(source: &Ed25519PublicKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Hex::<formats::Lowercase>::serialize_as(source.as_ref(), serializer)
    }
}

impl<'de> DeserializeAs<'de, Ed25519PublicKey> for PublicKeyFormat {
    fn deserialize_as<D>(deserializer: D) -> Result<Ed25519PublicKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data: Vec<u8> = Hex::<formats::Lowercase>::deserialize_as(deserializer)?;
        Ok(Ed25519PublicKey(data.as_slice().try_into().map_err(serde::de::Error::custom)?))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetadata {
    pub target_host: String,
    pub tags: HashSet<String>,
    pub skip: bool,
    pub sops_files: Vec<Utf8PathBuf>,
}

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemState {
    #[serde_as(as = "Option<PublicKeyFormat>")]
    pub public_key: Option<Ed25519PublicKey>,
}

#[derive(Debug)]
struct SystemInner {
    name: String,
    state: RwLock<SystemState>,
    metadata: OnceCell<SystemMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct System<'a, const Metadata: bool>(ByAddress<&'a SystemInner>);

impl<'a> System<'a, false> {
    pub fn with_metadata(&self) -> Result<System<'a, true>> {
        self.0.metadata.get_or_try_init(|| {
            nix_eval(format!("systemMetadata.{}", self.name()))
                .map_err(ErrorStyle::system_and_action(self.name(), "resolve metadata"))
        })?;
        Ok(System(self.0))
    }
}

impl<'a> System<'a, true> {
    pub fn metadata(&self) -> &'a SystemMetadata {
        self.0.metadata.get().unwrap()
    }
}

impl<'a, const Metadata: bool> System<'a, Metadata> {
    pub fn name(&self) -> &'a str {
        &self.0.name
    }

    pub fn state(&self) -> &'a RwLock<SystemState> {
        &self.0.state
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlakeMetadata {
    pub path: Utf8PathBuf,
}

impl FlakeMetadata {
    pub fn strip_store_path<'a>(&self, path: &'a Utf8Path) -> Result<&'a Utf8Path> {
        trace!(%self.path, %path, "resolve flake path");

        (|| {
            let path = path
                .strip_prefix(&config().eval_store)
                .or_else(|_| path.strip_prefix(RootDir))
                .context("path is not absolute")?;

            path.strip_prefix(
                self.path.strip_prefix(RootDir).context("flake path is not absolute")?,
            )
            .context("path is not inside the flake")
        })()
        .with_context(|| format!("resolve flake path '{path}'"))
    }

    pub fn flake_abs_path(&self, path: &Utf8Path) -> Result<Utf8PathBuf> {
        Ok(Utf8Path::new("flake").join(path))
    }
}

#[derive(Debug)]
pub struct Flake {
    systems: HashMap<String, SystemInner>,
    stale_state: HashMap<String, SystemState>,
    pub metadata: FlakeMetadata,
}

fn load_system_state() -> Result<HashMap<String, SystemState>> {
    let data = match fs::read(STATE_FILE) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(err).context(format!("read {STATE_FILE}")),
    };

    serde_json::from_slice(&data).context(format!("parse {STATE_FILE}"))
}

pub fn resolve_systems_metadata<'a>(
    systems: &[System<'a, false>],
) -> Result<Vec<System<'a, true>>> {
    scope(|s| {
        systems
            .iter()
            .map(|system| s.spawn(move || system.with_metadata()))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|j| j.join().unwrap_or(Err(eyre!("join failed"))))
            .collect()
    })
}

impl Flake {
    pub fn load() -> Result<Self> {
        let data = run_command_tty(Command::new("nix").tap_mut(|cmd| {
            cmd.args(["flake", "metadata", "--store"])
                .arg(&config().eval_store)
                .args(["--json", "path:./flake"]);
        }))
        .context("nix flake metadata")?;
        let flake_metadata: FlakeMetadata =
            serde_json::from_str(data.to_str().context("decode utf8 flake metadata")?)
                .context("parse flake metadata")?;

        let system_names = nix_eval::<Vec<String>>("systemNames").context("eval systemNames")?;
        let mut system_states = load_system_state().context("load system state")?;

        Ok(Self {
            metadata: flake_metadata,
            systems: system_names
                .into_iter()
                .map(|name| {
                    (
                        name.clone(),
                        SystemInner {
                            state: RwLock::new(system_states.remove(&name).unwrap_or_default()),
                            metadata: OnceCell::new(),
                            name,
                        },
                    )
                })
                .collect(),
            stale_state: system_states,
        })
    }

    pub fn get_system(&self, name: &str) -> Result<System<'_, false>> {
        self.systems.get(name).map(ByAddress).map(System).ok_or(eyre!("unknown system '{name}'"))
    }

    pub fn get_systems(&self) -> Vec<System<'_, false>> {
        self.systems.values().map(ByAddress).map(System).collect()
    }

    pub fn save_system_state(self) -> Result<()> {
        let mut state = self
            .systems
            .into_values()
            .map(|s| (s.name, s.state.into_inner()))
            .collect::<HashMap<_, _>>();
        state.extend(self.stale_state);

        let json = serde_json::to_string_pretty(&state).context("serialize state")?;
        fs::write(STATE_FILE, json).context(format!("write {STATE_FILE}"))?;

        Ok(())
    }
}
