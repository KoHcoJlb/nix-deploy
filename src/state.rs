use crate::{Cli, flake::Flake};

#[derive(derive_more::Debug)]
pub struct CliState<'a> {
    pub cli: &'a Cli,
    pub flake: Flake,
}
