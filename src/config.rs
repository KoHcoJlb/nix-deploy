use std::{fs, path::Path};

use camino::Utf8PathBuf;
use eyre::{Context, eyre};
use once_cell::sync::OnceCell;
use serde::Deserialize;
use serde_inline_default::serde_inline_default;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SopsConfig {
    pub management_keys: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SshConfig {
    pub config_file: Option<Utf8PathBuf>,
}

#[serde_inline_default]
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde_inline_default("auto".into())]
    pub eval_store: Utf8PathBuf,
    #[serde(default)]
    pub sops: SopsConfig,
    #[serde(default)]
    pub ssh: SshConfig,
}

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn load_config(path: impl AsRef<Path>) -> eyre::Result<()> {
    let data = fs::read_to_string(&path).context("read config")?;

    let mut config: Config = toml::from_str(&data)?;

    if config.eval_store != "auto" {
        config.eval_store =
            config.eval_store.canonicalize_utf8().context("canonicalize eval_store")?;
    }

    CONFIG.set(config).map_err(|_| eyre!("config already initialized"))?;

    Ok(())
}

pub fn config() -> &'static Config {
    CONFIG.get().unwrap()
}
