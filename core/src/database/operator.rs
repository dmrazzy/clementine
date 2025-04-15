//! # Operator Related Database Operations
//!
//! This module includes database functions which are mainly used by an operator.

use super::{
    wrapper::{
        AddressDB, DepositParamsDB, OutPointDB, SignaturesDB, TxOutDB, TxidDB, UtxoDB,
        XOnlyPublicKeyDB,
    },
    Database, DatabaseTransaction,
};
use crate::builder::transaction::{DepositData, KickoffData, OperatorData};
use crate::{
    errors::BridgeError,
    execute_query_with_tx,
    operator::PublicHash,
    rpc::clementine::{DepositSignatures, TaggedSignature},
    UTXO,
};
use bitcoin::{OutPoint, Txid, XOnlyPublicKey};
use bitvm::signatures::winternitz;
use bitvm::signatures::winternitz::PublicKey as WinternitzPublicKey;
use eyre::Context;
use std::str::FromStr;

pub type RootHash = [u8; 32];
//pub type PublicInputWots = Vec<[u8; 20]>;
pub type AssertTxHash = Vec<[u8; 32]>;

pub type BitvmSetup = (AssertTxHash, RootHash);

impl Database {
    /// TODO: wallet_address should have `Address` type.
    pub async fn set_operator(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        xonly_pubkey: XOnlyPublicKey,
        wallet_address: String,
        collateral_funding_outpoint: OutPoint,
    ) -> Result<(), BridgeError> {
        let query = sqlx::query(
            "INSERT INTO operators (xonly_pk, wallet_reimburse_address, collateral_funding_outpoint) VALUES ($1, $2, $3)
                    ON CONFLICT DO NOTHING;",
        )
        .bind(XOnlyPublicKeyDB(xonly_pubkey))
        .bind(wallet_address)
        .bind(OutPointDB(collateral_funding_outpoint));

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    pub async fn get_operators(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
    ) -> Result<Vec<(XOnlyPublicKey, bitcoin::Address, OutPoint)>, BridgeError> {
        let query = sqlx::query_as(
            "SELECT xonly_pk, wallet_reimburse_address, collateral_funding_outpoint FROM operators;"
        );

        let operators: Vec<(XOnlyPublicKeyDB, AddressDB, OutPointDB)> =
            execute_query_with_tx!(self.connection, tx, query, fetch_all)?;

        // Convert the result to the desired format
        let data = operators
            .into_iter()
            .map(|(pk, addr, outpoint_db)| {
                let xonly_pk = pk.0;
                let addr = addr.0.assume_checked();
                let outpoint = outpoint_db.0; // Extract the Txid from TxidDB
                Ok((xonly_pk, addr, outpoint))
            })
            .collect::<Result<Vec<_>, BridgeError>>()?;
        Ok(data)
    }

    pub async fn get_operator(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
    ) -> Result<Option<OperatorData>, BridgeError> {
        let query = sqlx::query_as(
            "SELECT xonly_pk, wallet_reimburse_address, collateral_funding_outpoint FROM operators WHERE xonly_pk = $1;"
        ).bind(XOnlyPublicKeyDB(operator_xonly_pk));

        let result: Option<(String, String, OutPointDB)> =
            execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            None => Ok(None),
            Some((_, addr, outpoint_db)) => {
                // Convert the result to the desired format
                let addr = bitcoin::Address::from_str(&addr)
                    .wrap_err("Invalid Address")?
                    .assume_checked();
                let outpoint = outpoint_db.0; // Extract the Txid from TxidDB
                Ok(Some(OperatorData {
                    xonly_pk: operator_xonly_pk,
                    reimburse_addr: addr,
                    collateral_funding_outpoint: outpoint,
                }))
            }
        }
    }

    /// Sets the funding UTXO for kickoffs.
    pub async fn set_funding_utxo(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        funding_utxo: UTXO,
    ) -> Result<(), BridgeError> {
        let query = sqlx::query("INSERT INTO funding_utxos (funding_utxo) VALUES ($1);").bind(
            sqlx::types::Json(UtxoDB {
                outpoint_db: OutPointDB(funding_utxo.outpoint),
                txout_db: TxOutDB(funding_utxo.txout),
            }),
        );

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    /// Gets the funding UTXO for kickoffs
    pub async fn get_funding_utxo(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
    ) -> Result<Option<UTXO>, BridgeError> {
        let query =
            sqlx::query_as("SELECT funding_utxo FROM funding_utxos ORDER BY id DESC LIMIT 1;");

        let result: Result<(sqlx::types::Json<UtxoDB>,), sqlx::Error> =
            execute_query_with_tx!(self.connection, tx, query, fetch_one);

        match result {
            Ok((utxo_db,)) => Ok(Some(UTXO {
                outpoint: utxo_db.outpoint_db.0,
                txout: utxo_db.txout_db.0.clone(),
            })),
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(BridgeError::DatabaseError(e)),
        }
    }

    /// Sets the unspent kickoff sigs received from operators during initial setup.
    /// Sigs of each round are stored together in the same row.
    pub async fn set_unspent_kickoff_sigs(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        round_idx: usize,
        signatures: Vec<TaggedSignature>,
    ) -> Result<(), BridgeError> {
        let query = sqlx::query(
            "INSERT INTO unspent_kickoff_signatures (xonly_pk, round_idx, signatures) VALUES ($1, $2, $3)
             ON CONFLICT (xonly_pk, round_idx) DO UPDATE
             SET signatures = EXCLUDED.signatures;",
        ).bind(XOnlyPublicKeyDB(operator_xonly_pk)).bind(round_idx as i32).bind(SignaturesDB(DepositSignatures{signatures}));

        execute_query_with_tx!(self.connection, tx, query, execute)?;
        Ok(())
    }

    /// Get unspent kickoff sigs for a specific operator and round.
    pub async fn get_unspent_kickoff_sigs(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        round_idx: usize,
    ) -> Result<Option<Vec<TaggedSignature>>, BridgeError> {
        let query = sqlx::query_as::<_, (SignaturesDB,)>("SELECT signatures FROM unspent_kickoff_signatures WHERE xonly_pk = $1 AND round_idx = $2;")
            .bind(XOnlyPublicKeyDB(operator_xonly_pk))
            .bind(round_idx as i32);

        let result: Result<(SignaturesDB,), sqlx::Error> =
            execute_query_with_tx!(self.connection, tx, query, fetch_one);

        match result {
            Ok((SignaturesDB(signatures),)) => Ok(Some(signatures.signatures)),
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(BridgeError::DatabaseError(e)),
        }
    }

    /// Sets Winternitz public keys (only for kickoff blockhash commit) for an operator.
    pub async fn set_operator_kickoff_winternitz_public_keys(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        winternitz_public_key: Vec<WinternitzPublicKey>,
    ) -> Result<(), BridgeError> {
        let wpk = borsh::to_vec(&winternitz_public_key).wrap_err(BridgeError::BorshError)?;

        let query = sqlx::query(
                "INSERT INTO operator_winternitz_public_keys (xonly_pk, winternitz_public_keys) VALUES ($1, $2);",
            )
            .bind(XOnlyPublicKeyDB(operator_xonly_pk))
            .bind(wpk);

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    /// Gets Winternitz public keys for every sequential collateral tx of an
    /// operator and a watchtower.
    pub async fn get_operator_kickoff_winternitz_public_keys(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        op_xonly_pk: XOnlyPublicKey,
    ) -> Result<Vec<winternitz::PublicKey>, BridgeError> {
        let query = sqlx::query_as(
                "SELECT winternitz_public_keys FROM operator_winternitz_public_keys WHERE xonly_pk = $1;",
            )
            .bind(XOnlyPublicKeyDB(op_xonly_pk));

        let wpks: (Vec<u8>,) = execute_query_with_tx!(self.connection, tx, query, fetch_one)?;

        let operator_winternitz_pks: Vec<winternitz::PublicKey> =
            borsh::from_slice(&wpks.0).wrap_err(BridgeError::BorshError)?;

        Ok(operator_winternitz_pks)
    }

    /// Sets public hashes for a specific operator, sequential collateral tx and
    /// kickoff index combination. If there is hashes for given indexes, they
    /// will be overwritten by the new hashes.
    pub async fn set_operator_challenge_ack_hashes(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        deposit_outpoint: OutPoint,
        public_hashes: &Vec<[u8; 20]>,
    ) -> Result<(), BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;
        let query = sqlx::query(
            "INSERT INTO operators_challenge_ack_hashes (xonly_pk, deposit_id, public_hashes)
             VALUES ($1, $2, $3)
             ON CONFLICT (xonly_pk, deposit_id) DO UPDATE
             SET public_hashes = EXCLUDED.public_hashes;",
        )
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?)
        .bind(public_hashes);

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    /// Retrieves public hashes for a specific operator, sequential collateral
    /// tx and kickoff index combination.
    pub async fn get_operators_challenge_ack_hashes(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        deposit_outpoint: OutPoint,
    ) -> Result<Option<Vec<PublicHash>>, BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;
        let query = sqlx::query_as::<_, (Vec<Vec<u8>>,)>(
            "SELECT public_hashes
            FROM operators_challenge_ack_hashes
            WHERE xonly_pk = $1 AND deposit_id = $2;",
        )
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?);

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((public_hashes,)) => {
                let mut converted_hashes = Vec::new();
                for hash in public_hashes {
                    match hash.try_into() {
                        Ok(public_hash) => converted_hashes.push(public_hash),
                        Err(err) => {
                            tracing::error!("Failed to convert hash: {:?}", err);
                            return Err(eyre::eyre!("Failed to convert public hash").into());
                        }
                    }
                }
                Ok(Some(converted_hashes))
            }
            None => Ok(None), // If no result is found, return Ok(None)
        }
    }

    /// Saves deposit infos, and returns the deposit_id
    pub async fn set_deposit_data(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_data: DepositData,
        move_to_vault_txid: Txid,
    ) -> Result<u32, BridgeError> {
        let query = sqlx::query_as(
            "INSERT INTO deposits (deposit_outpoint, deposit_params, move_to_vault_txid)
                VALUES ($1, $2, $3)
                ON CONFLICT (deposit_outpoint) DO UPDATE
                SET deposit_params = EXCLUDED.deposit_params,
                    move_to_vault_txid = EXCLUDED.move_to_vault_txid
                RETURNING deposit_id;
            ",
        )
        .bind(OutPointDB(deposit_data.get_deposit_outpoint()))
        .bind(DepositParamsDB(deposit_data.into()))
        .bind(TxidDB(move_to_vault_txid));

        let deposit_id: Result<(i32,), sqlx::Error> =
            execute_query_with_tx!(self.connection, tx, query, fetch_one);

        Ok(u32::try_from(deposit_id?.0).wrap_err("Failed to convert deposit id to u32")?)
    }

    pub async fn get_deposit_data_with_move_tx(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        move_to_vault_txid: Txid,
    ) -> Result<Option<DepositData>, BridgeError> {
        let query = sqlx::query_as::<_, (DepositParamsDB,)>(
            "SELECT deposit_params FROM deposits WHERE move_to_vault_txid = $1;",
        )
        .bind(TxidDB(move_to_vault_txid));

        let result: Option<(DepositParamsDB,)> =
            execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((deposit_params,)) => Ok(Some(
                deposit_params
                    .0
                    .try_into()
                    .map_err(|e| eyre::eyre!("Invalid deposit params {e}"))?,
            )),
            None => Ok(None),
        }
    }

    pub async fn get_deposit_data(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_outpoint: OutPoint,
    ) -> Result<Option<(u32, DepositData)>, BridgeError> {
        let query = sqlx::query_as(
            "SELECT deposit_id, deposit_params FROM deposits WHERE deposit_outpoint = $1;",
        )
        .bind(OutPointDB(deposit_outpoint));

        let result: Option<(i32, DepositParamsDB)> =
            execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((deposit_id, deposit_params)) => Ok(Some((
                u32::try_from(deposit_id).wrap_err("Failed to convert deposit id to u32")?,
                deposit_params
                    .0
                    .try_into()
                    .map_err(|e| eyre::eyre!("Invalid deposit params {e}"))?,
            ))),
            None => Ok(None),
        }
    }

    /// Saves the deposit signatures to the database for a single operator.
    /// The signatures array is identified by the deposit_outpoint and operator_idx.
    /// For the order of signatures, please check [`crate::builder::sighash::create_nofn_sighash_stream`]
    /// which determines the order of the sighashes that are signed.
    pub async fn set_deposit_signatures(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_outpoint: OutPoint,
        operator_xonly_pk: XOnlyPublicKey,
        round_idx: usize,
        kickoff_idx: usize,
        kickoff_txid: Txid,
        signatures: Vec<TaggedSignature>,
    ) -> Result<(), BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;

        let query = sqlx::query(
            "
            INSERT INTO deposit_signatures (deposit_id, operator_xonly_pk, round_idx, kickoff_idx, kickoff_txid, signatures)
            VALUES ($1, $2, $3, $4, $5, $6);"
        )
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?)
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(round_idx as i32)
        .bind(kickoff_idx as i32)
        .bind(TxidDB(kickoff_txid))
        .bind(SignaturesDB(DepositSignatures{signatures}));

        execute_query_with_tx!(self.connection, tx, query, execute)?;
        Ok(())
    }

    /// Gets a unique int for a deposit outpoint
    pub async fn get_deposit_id(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_outpoint: OutPoint,
    ) -> Result<u32, BridgeError> {
        let query = sqlx::query_as("INSERT INTO deposits (deposit_outpoint)
            VALUES ($1)
            ON CONFLICT (deposit_outpoint) DO UPDATE SET deposit_outpoint = deposits.deposit_outpoint
            RETURNING deposit_id;")
            .bind(OutPointDB(deposit_outpoint));

        let deposit_id: Result<(i32,), sqlx::Error> =
            execute_query_with_tx!(self.connection, tx, query, fetch_one);
        Ok(u32::try_from(deposit_id?.0).wrap_err("Failed to convert deposit id to u32")?)
    }

    /// Retrieves the deposit signatures for a single operator for a single reimburse
    /// process (single kickoff utxo).
    /// The signatures are tagged so that each signature can be matched with the correct
    /// txin it belongs to easily.
    pub async fn get_deposit_signatures(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_outpoint: OutPoint,
        operator_xonly_pk: XOnlyPublicKey,
        round_idx: usize,
        kickoff_idx: usize,
    ) -> Result<Option<Vec<TaggedSignature>>, BridgeError> {
        let query = sqlx::query_as::<_, (SignaturesDB,)>(
            "SELECT ds.signatures FROM deposit_signatures ds
                    INNER JOIN deposits d ON d.deposit_id = ds.deposit_id
                 WHERE d.deposit_outpoint = $1
                 AND ds.operator_xonly_pk = $2
                 AND ds.round_idx = $3
                 AND ds.kickoff_idx = $4;",
        )
        .bind(OutPointDB(deposit_outpoint))
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(round_idx as i32)
        .bind(kickoff_idx as i32);

        let result: Result<(SignaturesDB,), sqlx::Error> =
            execute_query_with_tx!(self.connection, tx, query, fetch_one);

        match result {
            Ok((SignaturesDB(signatures),)) => Ok(Some(signatures.signatures)),
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(BridgeError::DatabaseError(e)),
        }
    }

    pub async fn get_deposit_data_with_kickoff_txid(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        kickoff_txid: Txid,
    ) -> Result<Option<(DepositData, KickoffData)>, BridgeError> {
        let query = sqlx::query_as::<_, (DepositParamsDB, XOnlyPublicKeyDB, i32, i32)>(
            "SELECT d.deposit_params, ds.operator_xonly_pk, ds.round_idx, ds.kickoff_idx
             FROM deposit_signatures ds
             INNER JOIN deposits d ON d.deposit_id = ds.deposit_id
             WHERE ds.kickoff_txid = $1;",
        )
        .bind(TxidDB(kickoff_txid));

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((deposit_params, operator_xonly_pk, round_idx, kickoff_idx)) => Ok(Some((
                deposit_params.0.try_into()?,
                KickoffData {
                    operator_xonly_pk: operator_xonly_pk.0,
                    round_idx: u32::try_from(round_idx)
                        .wrap_err("Failed to convert round idx to u32")?,
                    kickoff_idx: u32::try_from(kickoff_idx)
                        .wrap_err("Failed to convert kickoff idx to u32")?,
                },
            ))),
            None => Ok(None),
        }
    }

    /// Saves BitVM setup data for a specific operator, sequential collateral tx and kickoff index combination
    pub async fn set_bitvm_setup(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        deposit_outpoint: OutPoint,
        assert_tx_addrs: impl AsRef<[[u8; 32]]>,
        root_hash: &[u8; 32],
    ) -> Result<(), BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;
        let query = sqlx::query(
            "INSERT INTO bitvm_setups (xonly_pk, deposit_id, assert_tx_addrs, root_hash)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (xonly_pk, deposit_id) DO UPDATE
             SET assert_tx_addrs = EXCLUDED.assert_tx_addrs,
                 root_hash = EXCLUDED.root_hash;",
        )
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?)
        .bind(
            assert_tx_addrs
                .as_ref()
                .iter()
                .map(|addr| addr.as_ref())
                .collect::<Vec<&[u8]>>(),
        )
        .bind(root_hash.to_vec());

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    /// Retrieves BitVM setup data for a specific operator, sequential collateral tx and kickoff index combination
    pub async fn get_bitvm_setup(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        deposit_outpoint: OutPoint,
    ) -> Result<Option<BitvmSetup>, BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;
        let query = sqlx::query_as::<_, (Vec<Vec<u8>>, Vec<u8>)>(
            "SELECT assert_tx_addrs, root_hash
             FROM bitvm_setups
             WHERE xonly_pk = $1 AND deposit_id = $2;",
        )
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?);

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((assert_tx_addrs, root_hash)) => {
                // Convert root_hash Vec<u8> back to [u8; 32]
                let mut root_hash_array = [0u8; 32];
                root_hash_array.copy_from_slice(&root_hash);

                let assert_tx_addrs: Vec<[u8; 32]> = assert_tx_addrs
                    .into_iter()
                    .map(|addr| {
                        let mut addr_array = [0u8; 32];
                        addr_array.copy_from_slice(&addr);
                        addr_array
                    })
                    .collect();

                Ok(Some((assert_tx_addrs, root_hash_array)))
            }
            None => Ok(None),
        }
    }

    /// Retrieves BitVM disprove scripts root hash data for a specific operator, sequential collateral tx and kickoff index combination
    pub async fn get_bitvm_root_hash(
        &self,
        mut tx: Option<DatabaseTransaction<'_, '_>>,
        operator_xonly_pk: XOnlyPublicKey,
        deposit_outpoint: OutPoint,
    ) -> Result<Option<RootHash>, BridgeError> {
        let deposit_id = self
            .get_deposit_id(tx.as_deref_mut(), deposit_outpoint)
            .await?;
        let query = sqlx::query_as::<_, (Vec<u8>,)>(
            "SELECT root_hash
             FROM bitvm_setups
             WHERE xonly_pk = $1 AND deposit_id = $2;",
        )
        .bind(XOnlyPublicKeyDB(operator_xonly_pk))
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?);

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((root_hash,)) => {
                // Convert root_hash Vec<u8> back to [u8; 32]
                let mut root_hash_array = [0u8; 32];
                root_hash_array.copy_from_slice(&root_hash);
                Ok(Some(root_hash_array))
            }
            None => Ok(None),
        }
    }

    pub async fn set_kickoff_connector_as_used(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        round_idx: u32,
        kickoff_connector_idx: u32,
        kickoff_txid: Option<Txid>,
    ) -> Result<(), BridgeError> {
        let query = sqlx::query(
            "INSERT INTO used_kickoff_connectors (round_idx, kickoff_connector_idx, kickoff_txid)
             VALUES ($1, $2, $3);",
        )
        .bind(i32::try_from(round_idx).wrap_err("Failed to convert round idx to i32")?)
        .bind(
            i32::try_from(kickoff_connector_idx)
                .wrap_err("Failed to convert kickoff connector idx to i32")?,
        )
        .bind(kickoff_txid.map(TxidDB));

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }

    pub async fn get_kickoff_txid_for_used_kickoff_connector(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        round_idx: u32,
        kickoff_connector_idx: u32,
    ) -> Result<Option<Txid>, BridgeError> {
        let query = sqlx::query_as::<_, (TxidDB,)>(
            "SELECT kickoff_txid FROM used_kickoff_connectors WHERE round_idx = $1 AND kickoff_connector_idx = $2;",
        )
        .bind(i32::try_from(round_idx).wrap_err("Failed to convert round idx to i32")?)
        .bind(i32::try_from(kickoff_connector_idx).wrap_err("Failed to convert kickoff connector idx to i32")?);

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((txid,)) => Ok(Some(txid.0)),
            None => Ok(None),
        }
    }

    pub async fn get_unused_and_signed_kickoff_connector(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        deposit_id: u32,
        operator_xonly_pk: XOnlyPublicKey,
    ) -> Result<Option<(u32, u32)>, BridgeError> {
        // TODO: check if AND ds.round_idx >= cr.round_idx is correct or if we should use = instead
        let query = sqlx::query_as::<_, (i32, i32)>(
            "WITH current_round AS (
                    SELECT round_idx
                    FROM current_round_index
                    WHERE id = 1
                )
                SELECT
                    ds.round_idx as round_idx,
                    ds.kickoff_idx as kickoff_connector_idx
                FROM deposit_signatures ds
                CROSS JOIN current_round cr
                WHERE ds.deposit_id = $1  -- Parameter for deposit_id
                    AND ds.operator_xonly_pk = $2
                    AND ds.round_idx >= cr.round_idx
                    AND NOT EXISTS (
                        SELECT 1
                        FROM used_kickoff_connectors ukc
                        WHERE ukc.round_idx = ds.round_idx
                        AND ukc.kickoff_connector_idx = ds.kickoff_idx
                    )
                ORDER BY ds.round_idx ASC
                LIMIT 1;",
        )
        .bind(i32::try_from(deposit_id).wrap_err("Failed to convert deposit id to i32")?)
        .bind(XOnlyPublicKeyDB(operator_xonly_pk));

        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;

        match result {
            Some((round_idx, kickoff_connector_idx)) => Ok(Some((
                u32::try_from(round_idx).wrap_err("Failed to convert round idx to u32")?,
                u32::try_from(kickoff_connector_idx)
                    .wrap_err("Failed to convert kickoff connector idx to u32")?,
            ))),
            None => Ok(None),
        }
    }

    pub async fn get_current_round_index(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
    ) -> Result<Option<u32>, BridgeError> {
        let query =
            sqlx::query_as::<_, (i32,)>("SELECT round_idx FROM current_round_index WHERE id = 1;");
        let result = execute_query_with_tx!(self.connection, tx, query, fetch_optional)?;
        match result {
            Some((round_idx,)) => Ok(Some(
                u32::try_from(round_idx).wrap_err("Failed to convert round idx to u32")?,
            )),
            None => Ok(None),
        }
    }

    pub async fn update_current_round_index(
        &self,
        tx: Option<DatabaseTransaction<'_, '_>>,
        round_idx: u32,
    ) -> Result<(), BridgeError> {
        let query = sqlx::query("UPDATE current_round_index SET round_idx = $1 WHERE id = 1;")
            .bind(i32::try_from(round_idx).wrap_err("Failed to convert round idx to i32")?);

        execute_query_with_tx!(self.connection, tx, query, execute)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::bitvm_client::{SECP, UNSPENDABLE_XONLY_PUBKEY};
    use crate::citrea::mock::MockCitreaClient;
    use crate::operator::Operator;
    use crate::rpc::clementine::{
        DepositSignatures, NormalSignatureKind, NumberedSignatureKind, TaggedSignature,
    };
    use crate::UTXO;
    use crate::{database::Database, test::common::*};
    use bitcoin::hashes::Hash;
    use bitcoin::key::constants::SCHNORR_SIGNATURE_SIZE;
    use bitcoin::key::Keypair;
    use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid, XOnlyPublicKey};

    // #[tokio::test]
    // async fn save_get_operators() {
    //     let config = create_test_config_with_thread_name().await;
    //     let database = Database::new(&config).await.unwrap();
    //     let mut ops = Vec::new();
    //     for i in 0..2 {
    //         let txid_str = format!(
    //             "16b3a5951cb816afeb9dab8a30d0ece7acd3a7b34437436734edd1b72b6bf0{:02x}",
    //             i
    //         );
    //         let txid = Txid::from_str(&txid_str).unwrap();
    //         ops.push((
    //             i,
    //             config.operators_xonly_pks[i],
    //             config.operator_wallet_addresses[i].clone(),
    //             OutPoint { txid, vout: 0 },
    //         ));
    //     }
    //     // add to db
    //     for x in ops.iter() {
    //         database
    //             .set_operator(
    //                 None,
    //                 x.0 as i32,
    //                 x.1,
    //                 x.2.clone().assume_checked().to_string(),
    //                 x.3,
    //             )
    //             .await
    //             .unwrap();
    //     }
    //     let res = database.get_operators(None).await.unwrap();
    //     assert_eq!(res.len(), ops.len());
    //     for i in 0..2 {
    //         assert_eq!(res[i].0, ops[i].1);
    //         assert_eq!(res[i].1, ops[i].2.clone().assume_checked());
    //         assert_eq!(res[i].2, ops[i].3);
    //     }

    //     let res_single = database.get_operator(None, 1).await.unwrap().unwrap();
    //     assert_eq!(res_single.xonly_pk, ops[1].1);
    //     assert_eq!(res_single.reimburse_addr, ops[1].2.clone().assume_checked());
    //     assert_eq!(res_single.collateral_funding_outpoint, ops[1].3);
    // }

    #[tokio::test]
    async fn test_save_get_public_hashes() {
        let config = create_test_config_with_thread_name().await;
        let database = Database::new(&config).await.unwrap();

        let public_hashes = vec![[1u8; 20], [2u8; 20]];

        let deposit_outpoint = OutPoint {
            txid: Txid::from_byte_array([1u8; 32]),
            vout: 0,
        };

        let operator_xonly_pk = generate_random_xonly_pk();
        let non_existant_xonly_pk = generate_random_xonly_pk();

        // Save public hashes
        database
            .set_operator_challenge_ack_hashes(
                None,
                operator_xonly_pk,
                deposit_outpoint,
                &public_hashes.clone(),
            )
            .await
            .unwrap();

        // Retrieve and verify
        let result = database
            .get_operators_challenge_ack_hashes(None, operator_xonly_pk, deposit_outpoint)
            .await
            .unwrap();

        assert_eq!(result, Some(public_hashes));

        // Test non-existent entry
        let non_existent = database
            .get_operators_challenge_ack_hashes(None, non_existant_xonly_pk, deposit_outpoint)
            .await
            .unwrap();
        assert!(non_existent.is_none());
    }

    #[tokio::test]
    async fn test_save_get_unspent_kickoff_sigs() {
        let config = create_test_config_with_thread_name().await;
        let database = Database::new(&config).await.unwrap();

        let round_idx = 1;
        let signatures = DepositSignatures {
            signatures: vec![
                TaggedSignature {
                    signature_id: Some((NumberedSignatureKind::UnspentKickoff1, 1).into()),
                    signature: vec![0x1F; SCHNORR_SIGNATURE_SIZE],
                },
                TaggedSignature {
                    signature_id: Some((NumberedSignatureKind::UnspentKickoff2, 1).into()),
                    signature: (vec![0x2F; SCHNORR_SIGNATURE_SIZE]),
                },
                TaggedSignature {
                    signature_id: Some((NumberedSignatureKind::UnspentKickoff1, 2).into()),
                    signature: vec![0x1F; SCHNORR_SIGNATURE_SIZE],
                },
                TaggedSignature {
                    signature_id: Some((NumberedSignatureKind::UnspentKickoff2, 2).into()),
                    signature: (vec![0x2F; SCHNORR_SIGNATURE_SIZE]),
                },
            ],
        };

        let operator_xonly_pk = generate_random_xonly_pk();
        let non_existant_xonly_pk = generate_random_xonly_pk();

        database
            .set_unspent_kickoff_sigs(
                None,
                operator_xonly_pk,
                round_idx,
                signatures.signatures.clone(),
            )
            .await
            .unwrap();

        let result = database
            .get_unspent_kickoff_sigs(None, operator_xonly_pk, round_idx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, signatures.signatures);

        let non_existent = database
            .get_unspent_kickoff_sigs(None, non_existant_xonly_pk, round_idx)
            .await
            .unwrap();
        assert!(non_existent.is_none());

        let non_existent = database
            .get_unspent_kickoff_sigs(None, non_existant_xonly_pk, round_idx + 1)
            .await
            .unwrap();
        assert!(non_existent.is_none());
    }

    #[tokio::test]
    async fn test_operators_funding_utxo_1() {
        let config = create_test_config_with_thread_name().await;
        let db = Database::new(&config).await.unwrap();

        let utxo = UTXO {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([1u8; 32]),
                vout: 1,
            },
            txout: TxOut {
                value: Amount::from_sat(100),
                script_pubkey: ScriptBuf::from(vec![1u8]),
            },
        };
        db.set_funding_utxo(None, utxo.clone()).await.unwrap();
        let db_utxo = db.get_funding_utxo(None).await.unwrap().unwrap();

        // Sanity check
        assert_eq!(db_utxo, utxo);
    }

    #[tokio::test]
    async fn test_operators_funding_utxo_2() {
        let config = create_test_config_with_thread_name().await;
        let db = Database::new(&config).await.unwrap();

        let db_utxo = db.get_funding_utxo(None).await.unwrap();

        assert!(db_utxo.is_none());
    }

    #[tokio::test]
    async fn test_save_get_bitvm_setup() {
        let config = create_test_config_with_thread_name().await;
        let database = Database::new(&config).await.unwrap();

        let assert_tx_hashes: Vec<[u8; 32]> = vec![[1u8; 32], [4u8; 32]];
        let root_hash = [42u8; 32];
        //let public_input_wots = vec![[1u8; 20], [2u8; 20]];

        let deposit_outpoint = OutPoint {
            txid: Txid::from_byte_array([1u8; 32]),
            vout: 0,
        };
        let operator_xonly_pk = generate_random_xonly_pk();
        let non_existant_xonly_pk = generate_random_xonly_pk();

        // Save BitVM setup
        database
            .set_bitvm_setup(
                None,
                operator_xonly_pk,
                deposit_outpoint,
                &assert_tx_hashes,
                &root_hash,
            )
            .await
            .unwrap();

        // Retrieve and verify
        let result = database
            .get_bitvm_setup(None, operator_xonly_pk, deposit_outpoint)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.0, assert_tx_hashes);
        assert_eq!(result.1, root_hash);

        let hash = database
            .get_bitvm_root_hash(None, operator_xonly_pk, deposit_outpoint)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hash, root_hash);

        // Test non-existent entry
        let non_existent = database
            .get_bitvm_setup(None, non_existant_xonly_pk, deposit_outpoint)
            .await
            .unwrap();
        assert!(non_existent.is_none());
    }

    #[tokio::test]
    async fn set_get_operator_winternitz_public_keys() {
        let mut config = create_test_config_with_thread_name().await;
        let database = Database::new(&config).await.unwrap();
        let _regtest = create_regtest_rpc(&mut config).await;

        let operator = Operator::<MockCitreaClient>::new(config.clone())
            .await
            .unwrap();
        let op_xonly_pk =
            XOnlyPublicKey::from_keypair(&Keypair::from_secret_key(&SECP, &config.secret_key)).0;
        let deposit_outpoint = OutPoint {
            txid: Txid::from_slice(&[0x45; 32]).unwrap(),
            vout: 0x1F,
        };
        let wpks = operator
            .generate_assert_winternitz_pubkeys(deposit_outpoint)
            .unwrap();

        database
            .set_operator_kickoff_winternitz_public_keys(None, op_xonly_pk, wpks.clone())
            .await
            .unwrap();

        let result = database
            .get_operator_kickoff_winternitz_public_keys(None, op_xonly_pk)
            .await
            .unwrap();
        assert_eq!(result, wpks);

        let non_existent = database
            .get_operator_kickoff_winternitz_public_keys(None, *UNSPENDABLE_XONLY_PUBKEY)
            .await;
        assert!(non_existent.is_err());
    }

    #[tokio::test]
    async fn set_get_deposit_signatures() {
        let config = create_test_config_with_thread_name().await;
        let database = Database::new(&config).await.unwrap();

        let operator_xonly_pk = generate_random_xonly_pk();
        let unset_operator_xonly_pk = generate_random_xonly_pk();
        let deposit_outpoint = OutPoint {
            txid: Txid::from_slice(&[0x45; 32]).unwrap(),
            vout: 0x1F,
        };
        let round_idx = 1;
        let kickoff_idx = 1;
        let signatures = DepositSignatures {
            signatures: vec![
                TaggedSignature {
                    signature_id: Some(NormalSignatureKind::Reimburse1.into()),
                    signature: vec![0x1F; SCHNORR_SIGNATURE_SIZE],
                },
                TaggedSignature {
                    signature_id: Some((NumberedSignatureKind::OperatorChallengeNack1, 1).into()),
                    signature: (vec![0x2F; SCHNORR_SIGNATURE_SIZE]),
                },
            ],
        };

        database
            .set_deposit_signatures(
                None,
                deposit_outpoint,
                operator_xonly_pk,
                round_idx,
                kickoff_idx,
                Txid::all_zeros(),
                signatures.signatures.clone(),
            )
            .await
            .unwrap();

        let result = database
            .get_deposit_signatures(
                None,
                deposit_outpoint,
                operator_xonly_pk,
                round_idx,
                kickoff_idx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, signatures.signatures);

        let non_existent = database
            .get_deposit_signatures(
                None,
                deposit_outpoint,
                operator_xonly_pk,
                round_idx + 1,
                kickoff_idx + 1,
            )
            .await
            .unwrap();
        assert!(non_existent.is_none());

        let non_existent = database
            .get_deposit_signatures(
                None,
                OutPoint::null(),
                unset_operator_xonly_pk,
                round_idx,
                kickoff_idx,
            )
            .await
            .unwrap();
        assert!(non_existent.is_none());
    }
}
