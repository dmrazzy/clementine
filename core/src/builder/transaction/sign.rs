use crate::actor::{Actor, WinternitzDerivationPath};
use crate::builder::transaction::{DepositId, TransactionType};
use crate::config::BridgeConfig;
use crate::constants::{WATCHTOWER_CHALLENGE_MESSAGE_LENGTH, WINTERNITZ_LOG_D};
use crate::database::Database;
use crate::errors::BridgeError;
use crate::rpc::clementine::{KickoffId, RawSignedTx};
use crate::{builder, utils};
use bitcoin::XOnlyPublicKey;
use tonic::Status;
pub struct TransactionRequestData {
    pub deposit_id: DepositId,
    pub transaction_type: TransactionType,
    pub kickoff_id: KickoffId,
    pub commit_data: Vec<u8>,
}

pub async fn create_and_sign_tx(
    db: Database,
    signer: &Actor,
    config: BridgeConfig,
    nofn_xonly_pk: XOnlyPublicKey,
    transaction_data: TransactionRequestData,
) -> Result<RawSignedTx, BridgeError> {
    // Get all the watchtower challenge addresses for this operator. We have all of them here (for all the kickoff_utxos).
    // Optimize: Make this only return for a specific kickoff, but its only 40mb (33bytes * 60000 (kickoff per op?) * 20 (watchtower count)
    let watchtower_all_challenge_addresses = (0..config.num_watchtowers)
        .map(|i| {
            db.get_watchtower_challenge_addresses(
                None,
                i as u32,
                transaction_data.kickoff_id.operator_idx,
            )
        })
        .collect::<Vec<_>>();
    let watchtower_all_challenge_addresses =
        futures::future::try_join_all(watchtower_all_challenge_addresses).await?;

    // Collect the challenge Winternitz pubkeys for this specific kickoff_utxo.
    let watchtower_challenge_addresses = (0..config.num_watchtowers)
        .map(|i| {
            watchtower_all_challenge_addresses[i][transaction_data
                .kickoff_id
                .sequential_collateral_idx
                as usize
                * config.num_kickoffs_per_sequential_collateral_tx
                + transaction_data.kickoff_id.kickoff_idx as usize]
                .clone()
        })
        .collect::<Vec<_>>();

    // get operator data
    let operator_data = db
        .get_operator(None, transaction_data.kickoff_id.operator_idx as i32)
        .await?;

    let mut txhandlers = builder::transaction::create_txhandlers(
        db.clone(),
        config.clone(),
        transaction_data.deposit_id.clone(),
        nofn_xonly_pk,
        transaction_data.transaction_type,
        transaction_data.kickoff_id,
        operator_data,
        Some(&watchtower_challenge_addresses),
        None,
    )
    .await?;

    let sig_query = db
        .get_deposit_signatures(
            None,
            transaction_data.deposit_id.deposit_outpoint,
            transaction_data.kickoff_id.operator_idx as usize,
            transaction_data.kickoff_id.sequential_collateral_idx as usize,
            transaction_data.kickoff_id.kickoff_idx as usize,
        )
        .await?;
    let signatures = sig_query.unwrap_or_default();

    let mut requested_txhandler = txhandlers
        .remove(&transaction_data.transaction_type)
        .ok_or(BridgeError::TxHandlerNotFound(
            transaction_data.transaction_type,
        ))?;

    signer.tx_sign_and_fill_sigs(&mut requested_txhandler, &signatures)?;

    if let TransactionType::OperatorChallengeAck(watchtower_idx) = transaction_data.transaction_type
    {
        let path = WinternitzDerivationPath {
            message_length: 1,
            log_d: 1,
            tx_type: crate::actor::TxType::OperatorChallengeACK,
            index: None,
            operator_idx: Some(transaction_data.kickoff_id.operator_idx),
            watchtower_idx: Some(watchtower_idx as u32),
            sequential_collateral_tx_idx: Some(
                transaction_data.kickoff_id.sequential_collateral_idx,
            ),
            kickoff_idx: Some(transaction_data.kickoff_id.kickoff_idx),
            intermediate_step_name: None,
        };
        let preimage = signer.generate_preimage_from_path(path)?;
        signer.tx_sign_preimage(&mut requested_txhandler, preimage)?;
    }
    if let TransactionType::MiniAssert(assert_idx) = transaction_data.transaction_type {
        let path = WinternitzDerivationPath {
            message_length: *utils::BITVM_CACHE
                .intermediate_variables
                .iter()
                .nth(assert_idx)
                .ok_or_else(|| Status::invalid_argument("Mini Assert Index is too big"))?
                .1 as u32,
            log_d: WINTERNITZ_LOG_D,
            tx_type: crate::actor::TxType::BitVM,
            index: Some(transaction_data.kickoff_id.operator_idx),
            operator_idx: None,
            watchtower_idx: None,
            sequential_collateral_tx_idx: Some(
                transaction_data.kickoff_id.sequential_collateral_idx,
            ),
            kickoff_idx: Some(transaction_data.kickoff_id.kickoff_idx),
            intermediate_step_name: Some(
                utils::BITVM_CACHE
                    .intermediate_variables
                    .iter()
                    .nth(assert_idx)
                    .ok_or_else(|| Status::invalid_argument("Mini Assert Index is too big"))?
                    .0,
            ),
        };
        signer.tx_sign_winternitz(
            &mut requested_txhandler,
            &transaction_data.commit_data,
            path,
        )?;
    }
    if let TransactionType::WatchtowerChallenge(_) = transaction_data.transaction_type {
        // same path as get_watchtower_winternitz_public_keys()
        let path = WinternitzDerivationPath {
            message_length: WATCHTOWER_CHALLENGE_MESSAGE_LENGTH,
            log_d: WINTERNITZ_LOG_D,
            tx_type: crate::actor::TxType::WatchtowerChallenge,
            index: None,
            operator_idx: Some(transaction_data.kickoff_id.operator_idx),
            watchtower_idx: None,
            sequential_collateral_tx_idx: Some(
                transaction_data.kickoff_id.sequential_collateral_idx,
            ),
            kickoff_idx: Some(transaction_data.kickoff_id.kickoff_idx),
            intermediate_step_name: None,
        };
        signer.tx_sign_winternitz(
            &mut requested_txhandler,
            &transaction_data.commit_data,
            path,
        )?;
    }

    let checked_txhandler = requested_txhandler.promote()?;

    Ok(RawSignedTx {
        raw_tx: bitcoin::consensus::encode::serialize(checked_txhandler.get_cached_tx()),
    })
}
