use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use derive_more::{Display, Error};
use eyre::{Report, Result, bail};
use futures::{StreamExt, stream::FuturesUnordered};
use russh::{
    client,
    keys::{Algorithm, PublicKey, PublicKeyOrCertificate},
};
use tokio::{net::TcpStream, time::timeout};
use tracing::error;

use crate::flake::System;

pub async fn keyscan<'a>(
    systems: impl IntoIterator<Item = System<'a, true>>,
) -> HashMap<System<'a, true>, PublicKey> {
    struct ClientHandler;

    #[derive(Debug, Display, Error)]
    #[display("KeyReceived")]
    struct KeyReceived(#[error(ignore)] PublicKey);

    impl client::Handler for ClientHandler {
        type Error = Report;

        async fn check_server_key(
            &mut self, server_public_key: &PublicKeyOrCertificate,
        ) -> Result<bool, Self::Error> {
            Err(KeyReceived(server_public_key.public_key()))?
        }
    }

    let mut config = client::Config::default();
    config.preferred.key = Cow::Borrowed(&[Algorithm::Ed25519]);
    let config = Arc::new(config);

    systems
        .into_iter()
        .map(async |system| {
            (
                system,
                async {
                    let stream = timeout(
                        Duration::from_secs(10),
                        TcpStream::connect((
                            system.metadata().target_host.as_str(),
                            system.metadata().target_port,
                        )),
                    )
                    .await??;
                    if let Err(err) =
                        client::connect_stream(config.clone(), stream, ClientHandler).await
                    {
                        return match err.downcast::<KeyReceived>() {
                            Ok(key) => Ok(key.0),
                            Err(err) => Err(err),
                        };
                    }
                    bail!("didn't receive key")
                }
                .await,
            )
        })
        .collect::<FuturesUnordered<_>>()
        .filter_map(async |(system, res)| match res {
            Ok(key) => Some((system, key)),
            Err(err) => {
                error!(system = system.name(), ?err, "fetch ssh key");
                None
            }
        })
        .collect()
        .await
}
