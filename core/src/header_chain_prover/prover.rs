//! # Prover
//!
//! Prover is responsible for preparing RiscZero header chain prover proofs.

use crate::{errors::BridgeError, header_chain_prover::HeaderChainProver};
use circuits::header_chain::{
    BlockHeaderCircuitOutput, CircuitBlockHeader, HeaderChainCircuitInput, HeaderChainPrevProofType,
};
use lazy_static::lazy_static;
use risc0_zkvm::{compute_image_id, ExecutorEnv, Receipt};
use std::time::Duration;
use tokio::time::sleep;

// Prepare prover binary and calculate it's image id, before anything else.
const ELF: &[u8; 195724] = include_bytes!("../../../scripts/header-chain-guest-regtest");
lazy_static! {
    static ref IMAGE_ID: [u32; 8] = compute_image_id(ELF)
        .unwrap()
        .as_words()
        .try_into()
        .unwrap();
}

impl HeaderChainProver {
    /// Proves given block headers.
    ///
    /// # Parameters
    ///
    /// - `prev_receipt`: Previous proof's receipt, if not genesis block
    /// - `block_headers`: Block headers to prove
    ///
    /// # Returns
    ///
    /// - [`Receipt`]: Proved block headers' proof receipt.
    fn prove_block_headers(
        &self,
        prev_receipt: Option<Receipt>,
        block_headers: Vec<CircuitBlockHeader>,
    ) -> Result<Receipt, BridgeError> {
        // Prepare proof input.
        let (prev_proof, method_id) = match &prev_receipt {
            Some(receipt) => {
                let prev_output: BlockHeaderCircuitOutput =
                    borsh::from_slice(&receipt.journal.bytes)
                        .map_err(BridgeError::ProverDeSerializationError)?;
                let method_id = prev_output.method_id;

                (HeaderChainPrevProofType::PrevProof(prev_output), method_id)
            }
            None => (HeaderChainPrevProofType::GenesisBlock, *IMAGE_ID),
        };
        let input = HeaderChainCircuitInput {
            method_id,
            prev_proof,
            block_headers,
        };

        let mut env = ExecutorEnv::builder();

        env.write_slice(&borsh::to_vec(&input).map_err(BridgeError::BorschError)?);

        if let Some(prev_receipt) = prev_receipt {
            env.add_assumption(prev_receipt);
        }

        let env = env
            .build()
            .map_err(|e| BridgeError::ProverError(format!("Can't build environment: {}", e)))?;

        let prover = risc0_zkvm::default_prover();

        tracing::trace!("Proving started for block");
        let receipt = prover
            .prove(env, ELF)
            .map_err(|e| BridgeError::ProverError(format!("Error while running prover: {}", e)))?
            .receipt;

        tracing::debug!("Proof receipt: {:?}", receipt);

        Ok(receipt)
    }

    /// Starts an async task that checks for non proved blocks and proves them.
    ///
    /// # Parameters
    ///
    /// - prover: [`ChainProver`] instance
    #[tracing::instrument(skip_all)]
    pub async fn start_prover(prover: HeaderChainProver) {
        loop {
            let non_proved_block = prover.db.get_non_proven_block(None).await;

            if let Ok((
                current_block_hash,
                current_block_header,
                _current_block_height,
                previous_proof,
            )) = non_proved_block
            {
                tracing::trace!(
                    "Prover starts proving for block with hash: {}",
                    current_block_hash
                );

                let header: CircuitBlockHeader = current_block_header.into();
                let receipt =
                    prover.prove_block_headers(Some(previous_proof), vec![header.clone()]);

                match receipt {
                    Ok(receipt) => {
                        prover
                            .db
                            .save_block_proof(None, current_block_hash, receipt)
                            .await
                            .unwrap();
                    }
                    Err(e) => {
                        tracing::error!("Can't prove for header {:?}: {}", header, e)
                    }
                };
            }

            sleep(Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::BridgeConfig, database::Database, initialize_database, utils::initialize_logger,
    };
    use crate::{
        create_test_config_with_thread_name, extended_rpc::ExtendedRpc,
        header_chain_prover::HeaderChainProver,
    };
    use bitcoin::{
        block::{Header, Version},
        hashes::Hash,
        BlockHash, CompactTarget, TxMerkleNode,
    };
    use bitcoincore_rpc::RpcApi;
    use circuits::header_chain::{BlockHeaderCircuitOutput, CircuitBlockHeader};
    use std::{env, thread};

    async fn mine_and_get_first_n_block_headers(
        rpc: ExtendedRpc,
        block_num: u64,
    ) -> Vec<CircuitBlockHeader> {
        let height = rpc.client.get_block_count().await.unwrap();
        if height < block_num {
            rpc.mine_blocks(block_num - height).await.unwrap();
        }

        let mut headers = Vec::new();
        for i in 0..block_num {
            let hash = rpc.client.get_block_hash(i).await.unwrap();
            let header = rpc.client.get_block_header(&hash).await.unwrap();

            headers.push(CircuitBlockHeader::from(header));
        }

        headers
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prove_block_headers_genesis() {
        let config = create_test_config_with_thread_name!(None);
        let rpc = ExtendedRpc::new(
            config.bitcoin_rpc_url.clone(),
            config.bitcoin_rpc_user.clone(),
            config.bitcoin_rpc_password.clone(),
        )
        .await;
        let prover = HeaderChainProver::new(&config, rpc.clone()).await.unwrap();

        let receipt = prover.prove_block_headers(None, vec![]).unwrap();

        let output: BlockHeaderCircuitOutput = borsh::from_slice(&receipt.journal.bytes).unwrap();
        println!("Proof journal output: {:?}", output);

        assert_eq!(output.chain_state.block_height, u32::MAX); // risc0-to-bitvm2 related
        assert_eq!(
            output.chain_state.best_block_hash,
            BlockHash::all_zeros().as_raw_hash().to_byte_array()
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prove_block_headers_second() {
        let config = create_test_config_with_thread_name!(None);
        let rpc = ExtendedRpc::new(
            config.bitcoin_rpc_url.clone(),
            config.bitcoin_rpc_user.clone(),
            config.bitcoin_rpc_password.clone(),
        )
        .await;
        let prover = HeaderChainProver::new(&config, rpc.clone()).await.unwrap();

        // Prove genesis block and get it's receipt.
        let receipt = prover.prove_block_headers(None, vec![]).unwrap();

        let block_headers = mine_and_get_first_n_block_headers(rpc, 3).await;
        let receipt = prover
            .prove_block_headers(Some(receipt), block_headers[0..2].to_vec())
            .unwrap();
        let output: BlockHeaderCircuitOutput = borsh::from_slice(&receipt.journal.bytes).unwrap();

        println!("Proof journal output: {:?}", output);

        assert_eq!(output.chain_state.block_height, 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn save_and_get_proof() {
        let config = create_test_config_with_thread_name!(None);
        let rpc = ExtendedRpc::new(
            config.bitcoin_rpc_url.clone(),
            config.bitcoin_rpc_user.clone(),
            config.bitcoin_rpc_password.clone(),
        )
        .await;
        let prover = HeaderChainProver::new(&config, rpc.clone()).await.unwrap();
        let block_headers = mine_and_get_first_n_block_headers(rpc, 3).await;

        // Prove genesis block.
        let receipt = prover.prove_block_headers(None, vec![]).unwrap();
        let hash =
            BlockHash::from_raw_hash(Hash::from_slice(&block_headers[1].prev_block_hash).unwrap());
        let header: Header = block_headers[0].clone().into();
        let _ = prover.db.save_new_block(None, hash, header, 0).await; // TODO: Unwrapping this causes errors.
        prover
            .db
            .save_block_proof(None, hash, receipt.clone())
            .await
            .unwrap();
        let database_receipt = prover.get_header_chain_proof(hash).await.unwrap();
        assert_eq!(receipt.journal, database_receipt.journal);
        assert_eq!(receipt.metadata, database_receipt.metadata);

        // Prove second block.
        let receipt = prover
            .prove_block_headers(Some(receipt), block_headers[0..2].to_vec())
            .unwrap();
        let hash =
            BlockHash::from_raw_hash(Hash::from_slice(&block_headers[2].prev_block_hash).unwrap());
        let header = Header {
            version: Version::from_consensus(block_headers[1].version),
            prev_blockhash: BlockHash::from_raw_hash(Hash::from_byte_array(
                block_headers[1].prev_block_hash,
            )),
            merkle_root: TxMerkleNode::from_raw_hash(Hash::from_byte_array(
                block_headers[1].merkle_root,
            )),
            time: block_headers[1].time,
            bits: CompactTarget::from_consensus(block_headers[1].bits),
            nonce: block_headers[1].nonce,
        };
        prover
            .db
            .save_new_block(None, hash, header, 0)
            .await
            .unwrap();

        prover
            .db
            .save_block_proof(None, hash, receipt.clone())
            .await
            .unwrap();
        let database_receipt2 = prover.get_header_chain_proof(hash).await.unwrap();
        assert_eq!(receipt.journal, database_receipt2.journal);
        assert_eq!(receipt.metadata, database_receipt2.metadata);
        assert_ne!(receipt.journal, database_receipt.journal);
    }
}
