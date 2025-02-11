use crate::actor::{Actor, WinternitzDerivationPath};
use crate::builder::script::WinternitzCommit;
use crate::builder::transaction::{DepositId, TransactionType, TxHandler};
use crate::config::BridgeConfig;
use crate::constants::{WATCHTOWER_CHALLENGE_MESSAGE_LENGTH, WINTERNITZ_LOG_D};
use crate::database::Database;
use crate::errors::BridgeError;
use crate::{builder, utils};
use bitcoin::XOnlyPublicKey;
use bitvm::signatures::winternitz;
use std::collections::HashMap;
use std::sync::Arc;
use crate::rpc::clementine::KickoffId;

fn get_txhandler<'a>(
    txhandlers: &'a HashMap<TransactionType, TxHandler>,
    tx_type: TransactionType,
) -> Result<&'a TxHandler, BridgeError> {
    txhandlers
        .get(&tx_type)
        .ok_or(BridgeError::TxHandlerNotFound(tx_type))
}

pub async fn create_txhandlers(
    db: Database,
    config: BridgeConfig,
    deposit: DepositId,
    nofn_xonly_pk: XOnlyPublicKey,
    transaction_type: TransactionType,
    kickoff_id: KickoffId,
    prev_reimburse_generator: Option<&TxHandler>,
    move_to_vault: Option<TxHandler>, // to not generate them if they were already generated
) -> Result<HashMap<TransactionType, TxHandler>, BridgeError> {
    let mut txhandlers = HashMap::new();
    // Create move_tx handler. This is unique for each deposit tx.
    let move_txhandler = match move_to_vault {
        Some(move_to_vault) => move_to_vault,
        None => builder::transaction::create_move_to_vault_txhandler(
            deposit.deposit_outpoint,
            deposit.evm_address,
            &deposit.recovery_taproot_address,
            nofn_xonly_pk,
            config.user_takes_after,
            config.bridge_amount_sats,
            config.network,
        )?,
    };
    txhandlers.insert(move_txhandler.get_transaction_type(), move_txhandler);

    // Get operator details (for each operator, (X-Only Public Key, Address, Collateral Funding Txid))
    let (operator_xonly_pk, operator_reimburse_address, collateral_funding_txid) =
        db.get_operator(None, kickoff_id.operator_idx as i32).await?;

    let (sequential_collateral_txhandler, reimburse_generator_txhandler) =
        match prev_reimburse_generator {
            Some(prev_reimburse_generator) => {
                let sequential_collateral_txhandler =
                    builder::transaction::create_sequential_collateral_txhandler(
                        operator_xonly_pk,
                        *prev_reimburse_generator.get_txid(),
                        prev_reimburse_generator
                            .get_spendable_output(0)?
                            .get_prevout()
                            .value,
                        config.timeout_block_count,
                        config.max_withdrawal_time_block_count,
                        config.num_kickoffs_per_sequential_collateral_tx,
                        config.network,
                    )?;

                // Create the reimburse_generator_tx handler.
                let reimburse_generator_txhandler =
                    builder::transaction::create_reimburse_generator_txhandler(
                        &sequential_collateral_txhandler,
                        operator_xonly_pk,
                        config.num_kickoffs_per_sequential_collateral_tx,
                        config.max_withdrawal_time_block_count,
                        config.network,
                    )?;
                (
                    sequential_collateral_txhandler,
                    reimburse_generator_txhandler,
                )
            }
            None => {
                // create nth sequential collateral tx and reimburse generator tx for the operator
                let (sequential_collateral_txhandler, reimburse_generator_txhandler) =
                    builder::transaction::create_seq_collat_reimburse_gen_nth_txhandler(
                        operator_xonly_pk,
                        collateral_funding_txid,
                        config.collateral_funding_amount,
                        config.timeout_block_count,
                        config.num_kickoffs_per_sequential_collateral_tx,
                        config.max_withdrawal_time_block_count,
                        config.network,
                        kickoff_id.sequential_collateral_idx as usize,
                    )?;
                (
                    sequential_collateral_txhandler,
                    reimburse_generator_txhandler,
                )
            }
        };

    txhandlers.insert(
        sequential_collateral_txhandler.get_transaction_type(),
        sequential_collateral_txhandler,
    );
    txhandlers.insert(
        reimburse_generator_txhandler.get_transaction_type(),
        reimburse_generator_txhandler,
    );

    let kickoff_txhandler = builder::transaction::create_kickoff_txhandler(
        get_txhandler(&txhandlers, TransactionType::SequentialCollateral)?,
        kickoff_id.kickoff_idx as usize,
        nofn_xonly_pk,
        operator_xonly_pk,
        *get_txhandler(&txhandlers, TransactionType::MoveToVault)?.get_txid(),
        kickoff_id.operator_idx as usize,
        config.network,
    )?;
    txhandlers.insert(kickoff_txhandler.get_transaction_type(), kickoff_txhandler);

    let kickoff_utxo_timeout_txhandler = builder::transaction::create_kickoff_utxo_timeout_txhandler(
        get_txhandler(&txhandlers, TransactionType::SequentialCollateral)?,
        kickoff_id.kickoff_idx as usize,
    )?;
    txhandlers.insert(
        kickoff_utxo_timeout_txhandler.get_transaction_type(),
        kickoff_utxo_timeout_txhandler,
    );

    // Creates the kickoff_timeout_tx handler.
    let kickoff_timeout_txhandler = builder::transaction::create_kickoff_timeout_txhandler(
        get_txhandler(&txhandlers, TransactionType::Kickoff)?,
        get_txhandler(&txhandlers, TransactionType::SequentialCollateral)?,
    )?;
    txhandlers.insert(
        kickoff_timeout_txhandler.get_transaction_type(),
        kickoff_timeout_txhandler,
    );

    // Creates the challenge_tx handler.
    let challenge_tx = builder::transaction::create_challenge_txhandler(
        get_txhandler(&txhandlers, TransactionType::Kickoff)?,
        &operator_reimburse_address,
    )?;
    txhandlers.insert(challenge_tx.get_transaction_type(), challenge_tx);

    // Generate Happy reimburse txs conditionally
    if matches!(
        transaction_type,
        TransactionType::StartHappyReimburse
            | TransactionType::HappyReimburse
            | TransactionType::AllNeededForVerifierDeposit
    ) {
        // Creates the start_happy_reimburse_tx handler.
        let start_happy_reimburse_txhandler =
            builder::transaction::create_start_happy_reimburse_txhandler(
                get_txhandler(&txhandlers, TransactionType::Kickoff)?,
                operator_xonly_pk,
                config.network,
            )?;
        txhandlers.insert(
            start_happy_reimburse_txhandler.get_transaction_type(),
            start_happy_reimburse_txhandler,
        );

        // Creates the happy_reimburse_tx handler.
        let happy_reimburse_txhandler = builder::transaction::create_happy_reimburse_txhandler(
            get_txhandler(&txhandlers, TransactionType::MoveToVault)?,
            get_txhandler(&txhandlers, TransactionType::StartHappyReimburse)?,
            get_txhandler(&txhandlers, TransactionType::ReimburseGenerator)?,
            kickoff_id.kickoff_idx as usize,
            &operator_reimburse_address,
        )?;
        txhandlers.insert(
            happy_reimburse_txhandler.get_transaction_type(),
            happy_reimburse_txhandler,
        );

        if !matches!(
            transaction_type,
            TransactionType::AllNeededForOperatorDeposit
                | TransactionType::AllNeededForVerifierDeposit
        ) {
            // We do not need other txhandlers, exit early
            return Ok(txhandlers);
        }
    }

    // Generate watchtower challenges (addresses from db) if all txs are needed
    if matches!(
        transaction_type,
        TransactionType::AllNeededForVerifierDeposit
            | TransactionType::WatchtowerChallengeKickoff
            | TransactionType::WatchtowerChallenge(_)
            | TransactionType::OperatorChallengeNack(_)
            | TransactionType::OperatorChallengeAck(_)
    ) {
        let needed_watchtower_idx: i32 =
            if let TransactionType::WatchtowerChallenge(idx) = transaction_type {
                idx as i32
            } else {
                -1
            };

        // Get all the watchtower challenge addresses for this operator. We have all of them here (for all the kickoff_utxos).
        // Optimize: Make this only return for a specific kickoff, but its only 40mb (33bytes * 60000 (kickoff per op?) * 20 (watchtower count)
        let watchtower_all_challenge_addresses = (0..config.num_watchtowers)
            .map(|i| db.get_watchtower_challenge_addresses(None, i as u32, kickoff_id.operator_idx))
            .collect::<Vec<_>>();
        let watchtower_all_challenge_addresses =
            futures::future::try_join_all(watchtower_all_challenge_addresses).await?;

        // Collect the challenge Winternitz pubkeys for this specific kickoff_utxo.
        let watchtower_challenge_addresses = (0..config.num_watchtowers)
            .map(|i| {
                watchtower_all_challenge_addresses[i][kickoff_id.sequential_collateral_idx as usize
                    * config.num_kickoffs_per_sequential_collateral_tx
                    + kickoff_id.kickoff_idx as usize]
                    .clone()
            })
            .collect::<Vec<_>>();

        let watchtower_challenge_kickoff_txhandler =
            builder::transaction::create_watchtower_challenge_kickoff_txhandler_from_db(
                get_txhandler(&txhandlers, TransactionType::Kickoff)?,
                config.num_watchtowers as u32,
                &watchtower_challenge_addresses,
            )?;
        txhandlers.insert(
            watchtower_challenge_kickoff_txhandler.get_transaction_type(),
            watchtower_challenge_kickoff_txhandler,
        );

        let public_hashes = db
            .get_operators_challenge_ack_hashes(
                None,
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            )
            .await?
            .ok_or(BridgeError::WatchtowerPublicHashesNotFound(
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            ))?;
        // Each watchtower will sign their Groth16 proof of the header chain circuit. Then, the operator will either
        // - acknowledge the challenge by sending the operator_challenge_ACK_tx, which will prevent the burning of the kickoff_tx.output[2],
        // - or do nothing, which will cause one to send the operator_challenge_NACK_tx, which will burn the kickoff_tx.output[2]
        // using watchtower_challenge_tx.output[0].
        for (watchtower_idx, public_hash) in public_hashes.iter().enumerate() {
            let watchtower_challenge_txhandler = if watchtower_idx as i32 != needed_watchtower_idx {
                // create it with db if we don't need actual winternitz script
                builder::transaction::create_watchtower_challenge_txhandler_from_db(
                    get_txhandler(&txhandlers, TransactionType::WatchtowerChallengeKickoff)?,
                    watchtower_idx,
                    public_hash,
                    nofn_xonly_pk,
                    operator_xonly_pk,
                    config.network,
                )?
            } else {
                // generate with actual scripts if we want to specifically create a watchtower challenge tx
                let path = WinternitzDerivationPath {
                    message_length: WATCHTOWER_CHALLENGE_MESSAGE_LENGTH,
                    log_d: WINTERNITZ_LOG_D,
                    tx_type: crate::actor::TxType::WatchtowerChallenge,
                    index: None,
                    operator_idx: Some(kickoff_id.operator_idx),
                    watchtower_idx: None,
                    sequential_collateral_tx_idx: Some(kickoff_id.sequential_collateral_idx),
                    kickoff_idx: Some(kickoff_id.kickoff_idx),
                    intermediate_step_name: None,
                };
                let actor = Actor::new(
                    config.secret_key,
                    config.winternitz_secret_key,
                    config.network,
                );
                let public_key = actor.derive_winternitz_pk(path)?;
                let winternitz_params = winternitz::Parameters::new(
                    WATCHTOWER_CHALLENGE_MESSAGE_LENGTH,
                    WINTERNITZ_LOG_D,
                );

                builder::transaction::create_watchtower_challenge_txhandler_from_script(
                    get_txhandler(&txhandlers, TransactionType::WatchtowerChallengeKickoff)?,
                    watchtower_idx,
                    public_hash,
                    Arc::new(WinternitzCommit::new(
                        public_key,
                        winternitz_params,
                        actor.xonly_public_key,
                    )),
                    nofn_xonly_pk,
                    operator_xonly_pk,
                    config.network,
                )?
            };
            txhandlers.insert(
                watchtower_challenge_txhandler.get_transaction_type(),
                watchtower_challenge_txhandler,
            );
            // Creates the operator_challenge_NACK_tx handler.
            let operator_challenge_nack_txhandler =
                builder::transaction::create_operator_challenge_nack_txhandler(
                    get_txhandler(&txhandlers, TransactionType::WatchtowerChallenge(watchtower_idx))?,
                    watchtower_idx,
                    get_txhandler(&txhandlers, TransactionType::Kickoff)?,
                )?;
            txhandlers.insert(
                operator_challenge_nack_txhandler.get_transaction_type(),
                operator_challenge_nack_txhandler,
            );

            if let TransactionType::OperatorChallengeAck(index) = transaction_type {
                // only create this if we specifically want to generate the Operator Challenge ACK tx
                if index == watchtower_idx {
                    let operator_challenge_ack_txhandler =
                        builder::transaction::create_operator_challenge_ack_txhandler(
                            get_txhandler(&txhandlers, TransactionType::WatchtowerChallenge(watchtower_idx))?,
                            watchtower_idx,
                        )?;

                    txhandlers.insert(
                        operator_challenge_ack_txhandler.get_transaction_type(),
                        operator_challenge_ack_txhandler,
                    );
                }
            }
        }
        if transaction_type != TransactionType::AllNeededForVerifierDeposit {
            // We do not need other txhandlers, exit early
            return Ok(txhandlers);
        }
    }

    // If we didn't return until this part, generate remaining assert/disprove tx's

    if matches!(
        transaction_type,
        TransactionType::AssertBegin | TransactionType::AssertEnd | TransactionType::MiniAssert(_)
    ) {
        // if we specifically want to generate assert txs, we need to generate correct Winternitz scripts
        let actor = Actor::new(
            config.secret_key,
            config.winternitz_secret_key,
            config.network,
        );
        let mut assert_scripts =
            Vec::with_capacity(utils::BITVM_CACHE.intermediate_variables.len());
        for (intermediate_step, intermediate_step_size) in
            utils::BITVM_CACHE.intermediate_variables.iter()
        {
            let params = winternitz::Parameters::new(*intermediate_step_size as u32 * 2, 4);
            let path = WinternitzDerivationPath {
                message_length: *intermediate_step_size as u32 * 2,
                log_d: 4,
                tx_type: crate::actor::TxType::BitVM,
                index: Some(kickoff_id.operator_idx), // same as in operator get_params, idk why its not operator_idx
                operator_idx: None,
                watchtower_idx: None,
                sequential_collateral_tx_idx: Some(kickoff_id.sequential_collateral_idx),
                kickoff_idx: Some(kickoff_id.kickoff_idx),
                intermediate_step_name: Some(intermediate_step),
            };
            let pk = actor.derive_winternitz_pk(path)?;
            assert_scripts.push(Arc::new(WinternitzCommit::new(
                pk,
                params,
                operator_xonly_pk,
            )));
        }
        // Creates the assert_begin_tx handler.
        let assert_begin_txhandler =
            builder::transaction::create_assert_begin_txhandler_from_scripts(
                get_txhandler(&txhandlers, TransactionType::Kickoff)?,
                &assert_scripts,
                config.network,
            )?;

        txhandlers.insert(
            assert_begin_txhandler.get_transaction_type(),
            assert_begin_txhandler,
        );

        let root_hash = db
            .get_bitvm_root_hash(
                None,
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            )
            .await?
            .ok_or(BridgeError::BitvmSetupNotFound(
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            ))?;

        // Creates the assert_end_tx handler.
        let mini_asserts_and_assert_end_txhandlers =
            builder::transaction::create_mini_asserts_and_assert_end_from_scripts(
                get_txhandler(&txhandlers, TransactionType::Kickoff)?,
                get_txhandler(&txhandlers, TransactionType::AssertBegin)?,
                &assert_scripts,
                &root_hash,
                nofn_xonly_pk,
                config.network,
            )?;
        for txhandler in mini_asserts_and_assert_end_txhandlers {
            txhandlers.insert(txhandler.get_transaction_type(), txhandler);
        }
    } else {
        // Get the bitvm setup for this operator, sequential collateral tx, and kickoff idx.
        let (assert_tx_addrs, root_hash, _public_input_wots) = db
            .get_bitvm_setup(
                None,
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            )
            .await?
            .ok_or(BridgeError::BitvmSetupNotFound(
                kickoff_id.operator_idx as i32,
                kickoff_id.sequential_collateral_idx as i32,
                kickoff_id.kickoff_idx as i32,
            ))?;

        // Creates the assert_begin_tx handler.
        let assert_begin_txhandler = builder::transaction::create_assert_begin_txhandler(
            get_txhandler(&txhandlers, TransactionType::Kickoff)?,
            &assert_tx_addrs,
            config.network,
        )?;

        txhandlers.insert(
            assert_begin_txhandler.get_transaction_type(),
            assert_begin_txhandler,
        );

        // Creates the assert_end_tx handler.
        let assert_end_txhandler = builder::transaction::create_assert_end_txhandler(
            get_txhandler(&txhandlers, TransactionType::Kickoff)?,
            get_txhandler(&txhandlers, TransactionType::AssertBegin)?,
            &assert_tx_addrs,
            &root_hash,
            nofn_xonly_pk,
            config.network,
        )?;

        txhandlers.insert(
            assert_end_txhandler.get_transaction_type(),
            assert_end_txhandler,
        );
    }

    // Creates the disprove_timeout_tx handler.
    let disprove_timeout_txhandler = builder::transaction::create_disprove_timeout_txhandler(
        get_txhandler(&txhandlers, TransactionType::AssertEnd)?,
        operator_xonly_pk,
        config.network,
    )?;

    txhandlers.insert(
        disprove_timeout_txhandler.get_transaction_type(),
        disprove_timeout_txhandler,
    );

    // Creates the already_disproved_tx handler.
    let already_disproved_txhandler = builder::transaction::create_already_disproved_txhandler(
        get_txhandler(&txhandlers, TransactionType::AssertEnd)?,
        get_txhandler(&txhandlers, TransactionType::SequentialCollateral)?,
    )?;

    txhandlers.insert(
        already_disproved_txhandler.get_transaction_type(),
        already_disproved_txhandler,
    );

    // Creates the reimburse_tx handler.
    let reimburse_txhandler = builder::transaction::create_reimburse_txhandler(
        get_txhandler(&txhandlers, TransactionType::MoveToVault)?,
        get_txhandler(&txhandlers, TransactionType::DisproveTimeout)?,
        get_txhandler(&txhandlers, TransactionType::ReimburseGenerator)?,
        kickoff_id.kickoff_idx as usize,
        &operator_reimburse_address,
    )?;

    txhandlers.insert(
        reimburse_txhandler.get_transaction_type(),
        reimburse_txhandler,
    );

    match transaction_type {
        TransactionType::AllNeededForOperatorDeposit => {
            let disprove_txhandler = builder::transaction::create_disprove_txhandler(
                get_txhandler(&txhandlers, TransactionType::AssertEnd)?,
                get_txhandler(&txhandlers, TransactionType::SequentialCollateral)?,
            )?;
            txhandlers.insert(
                disprove_txhandler.get_transaction_type(),
                disprove_txhandler,
            );
        }
        TransactionType::Disprove => {
            // TODO: if transactiontype::disprove, we need to add the actual disprove script here because requester wants to disprove the withdrawal
        }
        _ => {}
    }

    Ok(txhandlers)
}

#[cfg(test)]
mod tests {
    use crate::rpc::clementine::clementine_operator_client::ClementineOperatorClient;
    use crate::rpc::clementine::clementine_verifier_client::ClementineVerifierClient;
    use crate::rpc::clementine::clementine_watchtower_client::ClementineWatchtowerClient;
    use crate::{
        config::BridgeConfig,
        create_test_config_with_thread_name,
        database::Database,
        errors::BridgeError,
        initialize_database,
        rpc::clementine::DepositParams,
        servers::{
            create_aggregator_grpc_server, create_operator_grpc_server,
            create_verifier_grpc_server, create_watchtower_grpc_server,
        },
        utils::initialize_logger,
        EVMAddress,
    };
    use crate::{
        create_actors,
        extended_rpc::ExtendedRpc,
        rpc::clementine::{self, clementine_aggregator_client::ClementineAggregatorClient},
    };
    use bitcoin::Txid;

    use std::str::FromStr;
    use crate::builder::transaction::{DepositId, TransactionType};
    use crate::rpc::clementine::{KickoffId, GrpcTransactionId, TransactionRequest};

    #[tokio::test]
    #[serial_test::serial]
    async fn test_deposit_and_sign_txs() {
        let config = create_test_config_with_thread_name!(None);

        let (verifiers, mut operators, mut aggregator, watchtowers) = create_actors!(config);

        tracing::info!("Setting up aggregator");
        let start = std::time::Instant::now();

        aggregator
            .setup(tonic::Request::new(clementine::Empty {}))
            .await
            .unwrap();

        tracing::info!("Setup completed in {:?}", start.elapsed());
        tracing::info!("Depositing");
        let deposit_start = std::time::Instant::now();
        let deposit_outpoint = bitcoin::OutPoint {
            txid: Txid::from_str(
                "17e3fc7aae1035e77a91e96d1ba27f91a40a912cf669b367eb32c13a8f82bb02",
            )
            .unwrap(),
            vout: 0,
        };
        let recovery_taproot_address = bitcoin::Address::from_str(
            "tb1pk8vus63mx5zwlmmmglq554kwu0zm9uhswqskxg99k66h8m3arguqfrvywa",
        )
        .unwrap();
        let recovery_addr_checked = recovery_taproot_address.assume_checked();
        let evm_address = EVMAddress([1u8; 20]);

        let deposit_params = DepositParams {
            deposit_outpoint: Some(deposit_outpoint.clone().into()),
            evm_address: evm_address.0.to_vec(),
            recovery_taproot_address: recovery_addr_checked.to_string(),
        };

        aggregator
            .new_deposit(deposit_params.clone())
            .await
            .unwrap();
        tracing::info!("Deposit completed in {:?}", deposit_start.elapsed());

        let kickoff_id = KickoffId  {
            operator_idx: 0,
            sequential_collateral_idx: 0,
            kickoff_idx: 0,
        };

        let txs_operator_can_sign = vec![TransactionType::SequentialCollateral,
                                         TransactionType::ReimburseGenerator,
                                         TransactionType::Kickoff,
                                         TransactionType::Challenge,
                                         TransactionType::KickoffTimeout,
                                         TransactionType::KickoffUtxoTimeout,
                                         TransactionType::WatchtowerChallengeKickoff,
                                         TransactionType::StartHappyReimburse,
                                         TransactionType::HappyReimburse,
                                         TransactionType::OperatorChallengeNack(0),
                                         TransactionType::AssertBegin,
                                         //TransactionType::Disprove,
                                         TransactionType::DisproveTimeout,
                                         TransactionType::AlreadyDisproved,
                                         TransactionType::Reimburse,
                                         TransactionType::OperatorChallengeAck(0),
                                         TransactionType::MiniAssert(0),
                                         TransactionType::AssertEnd];

        for tx_type in txs_operator_can_sign {
            tracing::info!("Raw signed tx received for {:?}: {:?}", tx_type, operators[0].create_signed_tx(TransactionRequest {
                deposit_params: deposit_params.clone().into(),
                transaction_type: Some(GrpcTransactionId {
                    id: Some(tx_type.into()),
                }),
                kickoff_id: Some(kickoff_id),
            }).await.unwrap());
        }

    }
}
