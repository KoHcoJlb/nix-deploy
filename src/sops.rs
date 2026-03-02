use std::fs;

use bech32::{Bech32, Hrp};
use camino::Utf8Path;
use curve25519_dalek::edwards::CompressedEdwardsY;
use eyre::{Context, Result, ensure};
use russh::keys::ssh_key::public::Ed25519PublicKey;
use saphyr::{LoadableYamlNode, Yaml};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct KeyGroup {
    pub age: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CreationRule {
    pub path_regex: String,
    pub key_groups: Vec<KeyGroup>,
}

#[derive(Debug, Serialize)]
pub struct SopsYaml {
    pub creation_rules: Vec<CreationRule>,
}

pub fn public_key_to_age(key: Ed25519PublicKey) -> String {
    let point = CompressedEdwardsY::from_slice(key.as_ref()).unwrap().decompress().unwrap();

    let x25519 = point.to_montgomery();

    bech32::encode::<Bech32>(Hrp::parse_unchecked("age"), x25519.as_bytes()).unwrap()
}

pub fn is_encrypted(path: &Utf8Path) -> Result<bool> {
    let data = fs::read_to_string(path).context("read file")?;
    if data.lines().any(|l| l.starts_with("sops_version=")) {
        return Ok(true);
    }

    let yaml = Yaml::load_from_str(&data)?;
    Ok(yaml
        .first()
        .and_then(|y| y.as_mapping())
        .map(|h| h.contains_key(&Yaml::value_from_str("sops")))
        .unwrap_or(false))
}

pub fn verify_encrypted(path: &Utf8Path) -> Result<()> {
    ensure!(is_encrypted(path)?, "not encrypted");
    Ok(())
}
