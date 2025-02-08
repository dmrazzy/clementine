use crate::constants::{WATCHTOWER_CHALLENGE_MESSAGE_LENGTH, WINTERNITZ_LOG_D};
use crate::{
    actor::{Actor, WinternitzDerivationPath},
    builder::address::derive_challenge_address_from_xonlypk_and_wpk,
    config::BridgeConfig,
    database::Database,
    errors::BridgeError,
    extended_rpc::ExtendedRpc,
};
use bitcoin::ScriptBuf;
use bitvm::signatures::winternitz;

#[derive(Debug, Clone)]
pub struct Watchtower {
    _erpc: ExtendedRpc,
    _db: Database,
    pub actor: Actor,
    pub config: BridgeConfig,
}

impl Watchtower {
    pub async fn new(config: BridgeConfig) -> Result<Self, BridgeError> {
        let _erpc = ExtendedRpc::connect(
            config.bitcoin_rpc_url.clone(),
            config.bitcoin_rpc_user.clone(),
            config.bitcoin_rpc_password.clone(),
        )
        .await?;

        let _db = Database::new(&config).await?;
        let actor = Actor::new(
            config.secret_key,
            config.winternitz_secret_key,
            config.network,
        );

        Ok(Self {
            _erpc,
            _db,
            actor,
            config,
        })
    }

    /// Generates Winternitz public keys for every operator and sequential_collateral_tx pair and
    /// returns them.
    ///
    /// # Returns
    ///
    /// - [`Vec<Vec<winternitz::PublicKey>>`]: Winternitz public key for
    ///   `operator index` row and `sequential_collateral_tx index` column.
    pub async fn get_watchtower_winternitz_public_keys(
        &self,
    ) -> Result<Vec<winternitz::PublicKey>, BridgeError> {
        let mut winternitz_pubkeys = Vec::new();

        for operator in 0..self.config.num_operators as u32 {
            for sequential_collateral_tx in 0..self.config.num_sequential_collateral_txs as u32 {
                for kickoff_idx in 0..self.config.num_kickoffs_per_sequential_collateral_tx as u32 {
                    let path = WinternitzDerivationPath {
                        message_length: WATCHTOWER_CHALLENGE_MESSAGE_LENGTH,
                        log_d: WINTERNITZ_LOG_D,
                        tx_type: crate::actor::TxType::WatchtowerChallenge,
                        index: None,
                        operator_idx: Some(operator),
                        watchtower_idx: None,
                        sequential_collateral_tx_idx: Some(sequential_collateral_tx),
                        kickoff_idx: Some(kickoff_idx),
                        intermediate_step_name: None,
                    };

                    winternitz_pubkeys.push(self.actor.derive_winternitz_pk(path)?);
                }
            }
        }

        Ok(winternitz_pubkeys)
    }

    pub async fn get_watchtower_challenge_addresses(&self) -> Result<Vec<ScriptBuf>, BridgeError> {
        let mut challenge_addresses = Vec::new();

        let winternitz_pubkeys = self.get_watchtower_winternitz_public_keys().await?;
        tracing::info!(
            "get_watchtower_challenge_addresses watchtower xonly public key: {:?}",
            self.actor.xonly_public_key
        );
        tracing::info!(
            "get_watchtower_challenge_addresses watchtower taproot public key: {:?}",
            self.actor.address.script_pubkey()
        );
        for winternitz_pubkey in winternitz_pubkeys {
            let challenge_address = derive_challenge_address_from_xonlypk_and_wpk(
                &self.actor.xonly_public_key,
                &winternitz_pubkey,
                self.config.network,
            );
            challenge_addresses.push(challenge_address.script_pubkey());
        }

        Ok(challenge_addresses)
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::initialize_logger;
    use crate::watchtower::Watchtower;
    use crate::{
        config::BridgeConfig,
        database::Database,
        errors::BridgeError,
        extended_rpc::ExtendedRpc,
        initialize_database,
        servers::{
            create_aggregator_grpc_server, create_operator_grpc_server,
            create_verifier_grpc_server, create_watchtower_grpc_server,
        },
    };
    use crate::{create_actors, create_test_config_with_thread_name};
    use std::{env, thread};

    #[tokio::test]
    #[serial_test::serial]
    async fn new_watchtower() {
        let mut config = create_test_config_with_thread_name!(None);
        let (verifiers, operators, _, _should_not_panic) = create_actors!(config.clone());

        config.verifier_endpoints = Some(
            verifiers
                .iter()
                .map(|v| format!("http://{}", v.0))
                .collect(),
        );
        config.operator_endpoints = Some(
            operators
                .iter()
                .map(|o| format!("http://{}", o.0))
                .collect(),
        );

        let _should_not_panic = Watchtower::new(config.clone()).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn get_watchtower_winternitz_public_keys() {
        let mut config = create_test_config_with_thread_name!(None);
        let (verifiers, operators, _, _watchtowers) = create_actors!(config.clone());

        config.verifier_endpoints = Some(
            verifiers
                .iter()
                .map(|v| format!("http://{}", v.0))
                .collect(),
        );
        config.operator_endpoints = Some(
            operators
                .iter()
                .map(|o| format!("http://{}", o.0))
                .collect(),
        );

        let watchtower = Watchtower::new(config.clone()).await.unwrap();
        let watchtower_winternitz_public_keys = watchtower
            .get_watchtower_winternitz_public_keys()
            .await
            .unwrap();

        assert_eq!(
            watchtower_winternitz_public_keys.len(),
            config.num_operators
                * config.num_sequential_collateral_txs
                * config.num_kickoffs_per_sequential_collateral_tx
        );
    }
}
