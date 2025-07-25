//! # Circuits-lib - Header Chain Circuit
//! This module contains the implementation of the header chain circuit, which is basically
//! the Bitcoin header chain verification logic.
//!
//! Implementation of this module is inspired by the Bitcoin Core source code and from here:
//! https://github.com/ZeroSync/header_chain/tree/master/program/src/block_header.
//!
//! **⚠️ Warning:** This implementation is not a word-to-word translation of the Bitcoin Core source code.

use bitcoin::{
    block::{Header, Version},
    hashes::Hash,
    BlockHash, CompactTarget, TxMerkleNode,
};
use borsh::{BorshDeserialize, BorshSerialize};
use crypto_bigint::{Encoding, U256};
use mmr_guest::MMRGuest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

use crate::common::{get_network, zkvm::ZkvmGuest};

pub mod mmr_guest;
pub mod mmr_native;

/// The main entry point of the header chain circuit.
///
/// This function implements Bitcoin header chain verification logic within a zero-knowledge
/// virtual machine (zkVM) environment. It processes block headers, verifies chain continuity,
/// validates proof of work, and maintains the chain state.
///
/// ## Verification Process
///
/// The circuit performs several critical validations:
/// - **Method ID Consistency**: Ensures the input `method_id` matches any previous proof's `method_id`
/// - **Chain Continuity**: Confirms each block's `prev_block_hash` matches the `best_block_hash` of the preceding state
/// - **Block Hash Validity**: Calculates double SHA256 hash and checks it's ≤ current difficulty target
/// - **Difficulty Target Validation**: Verifies the `bits` field matches expected difficulty for current network/epoch
/// - **Timestamp Validation**: Ensures block timestamp > median of previous 11 block timestamps
/// - **MMR Integrity**: Maintains Merkle Mountain Range for efficient block hash storage and verification
///
/// ## Parameters
///
/// * `guest` - ZkvmGuest implementation for reading input, verifying proofs, and committing output
///
/// ## Input Format
///
/// Expects `HeaderChainCircuitInput` containing:
/// - `method_id`: Circuit version identifier
/// - `prev_proof`: Either genesis state or previous circuit output
/// - `block_headers`: Vector of block headers to process
///
/// ## Output Format
///
/// Commits `BlockHeaderCircuitOutput` containing:
/// - `method_id`: Same as input for consistency
/// - `genesis_state_hash`: Hash of initial chain state
/// - `chain_state`: Updated chain state after processing all headers
///
/// ## Panics
///
/// The function will panic on any validation failure including:
/// - Method ID mismatch between input and previous proof
/// - Invalid block hash (doesn't meet difficulty target)
/// - Chain discontinuity (prev_block_hash mismatch)
/// - Invalid timestamps
/// - Incorrect difficulty bits
pub fn header_chain_circuit(guest: &impl ZkvmGuest) {
    // Read the input from the host
    let input: HeaderChainCircuitInput = guest.read_from_host();
    let genesis_state_hash: [u8; 32];
    let mut chain_state = match input.prev_proof {
        HeaderChainPrevProofType::GenesisBlock(genesis_state) => {
            genesis_state_hash = genesis_state.to_hash();
            genesis_state
        }
        HeaderChainPrevProofType::PrevProof(prev_proof) => {
            assert_eq!(prev_proof.method_id, input.method_id, "Method ID mismatch, the input method ID must match the previous proof's method ID to ensure the same circuit is always used. Previous proof method ID: {:?}, input method ID: {:?}", prev_proof.method_id, input.method_id);
            guest.verify(input.method_id, &prev_proof);
            genesis_state_hash = prev_proof.genesis_state_hash;
            prev_proof.chain_state
        }
    };

    // Apply the block headers to the chain state
    chain_state.apply_block_headers(input.block_headers);

    // Commit the output to the host
    guest.commit(&BlockHeaderCircuitOutput {
        method_id: input.method_id,
        genesis_state_hash,
        chain_state,
    });
}

/// Network configuration holder for Bitcoin-specific constants.
///
/// Contains different representations of the maximum target for various Bitcoin networks
/// (mainnet, testnet4, signet, regtest). The maximum target defines the lowest possible
/// difficulty for the network.
///
/// ## Fields
///
/// * `max_bits` - Compact representation of maximum target (difficulty bits format)
/// * `max_target` - 256-bit representation of maximum target
/// * `max_target_bytes` - 32-byte array representation of maximum target
///
/// All three fields represent the same value in different formats for computational efficiency.
#[derive(Debug)]
pub struct NetworkConstants {
    pub max_bits: u32,
    pub max_target: U256,
    pub max_target_bytes: [u8; 32],
}

pub const NETWORK_TYPE: &str = get_network();

// Const evaluation of network type from environment
const IS_REGTEST: bool = matches!(NETWORK_TYPE.as_bytes(), b"regtest");
const IS_TESTNET4: bool = matches!(NETWORK_TYPE.as_bytes(), b"testnet4");
const MINIMUM_WORK_TESTNET: U256 =
    U256::from_be_hex("0000000000000000000000000000000000000000000000000000000100010001");

/// Network constants for the Bitcoin network configuration.
///
/// Determines the maximum target and difficulty bits based on the `BITCOIN_NETWORK`
/// environment variable. Supports mainnet, testnet4, signet, and regtest networks.
///
/// ## Network-Specific Values
///
/// - **Mainnet/Testnet4**: `max_bits = 0x1D00FFFF` (standard Bitcoin difficulty)
/// - **Signet**: `max_bits = 0x1E0377AE` (custom signet difficulty)
/// - **Regtest**: `max_bits = 0x207FFFFF` (minimal difficulty for testing)
///
/// Defaults to mainnet configuration if no environment variable is set.
pub const NETWORK_CONSTANTS: NetworkConstants = {
    match option_env!("BITCOIN_NETWORK") {
        Some(n) if matches!(n.as_bytes(), b"signet") => NetworkConstants {
            max_bits: 0x1E0377AE,
            max_target: U256::from_be_hex(
                "00000377AE000000000000000000000000000000000000000000000000000000",
            ),
            max_target_bytes: [
                0, 0, 3, 119, 174, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0,
            ],
        },
        Some(n) if matches!(n.as_bytes(), b"regtest") => NetworkConstants {
            max_bits: 0x207FFFFF,
            max_target: U256::from_be_hex(
                "7FFFFF0000000000000000000000000000000000000000000000000000000000",
            ),
            max_target_bytes: [
                127, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0,
            ],
        },
        Some(n) if matches!(n.as_bytes(), b"testnet4") => NetworkConstants {
            max_bits: 0x1D00FFFF,
            max_target: U256::from_be_hex(
                "00000000FFFF0000000000000000000000000000000000000000000000000000",
            ),
            max_target_bytes: [
                0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0,
            ],
        },
        Some(n) if matches!(n.as_bytes(), b"mainnet") => NetworkConstants {
            max_bits: 0x1D00FFFF,
            max_target: U256::from_be_hex(
                "00000000FFFF0000000000000000000000000000000000000000000000000000",
            ),
            max_target_bytes: [
                0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0,
            ],
        },
        // Default to mainnet for None
        None => NetworkConstants {
            max_bits: 0x1D00FFFF,
            max_target: U256::from_be_hex(
                "00000000FFFF0000000000000000000000000000000000000000000000000000",
            ),
            max_target_bytes: [
                0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0,
            ],
        },
        _ => panic!("Unsupported network"),
    }
};

/// Expected duration of a difficulty adjustment epoch in seconds.
///
/// Bitcoin adjusts difficulty every 2016 blocks (approximately 2 weeks).
/// - **Standard networks**: 2 weeks = 60 * 60 * 24 * 14 = 1,209,600 seconds
/// - **Custom signet**: Uses 10-second block time, so 60 * 24 * 14 = 20,160 seconds
///
/// See: <https://github.com/chainwayxyz/bitcoin/releases/tag/v29-ten-secs-blocktime-tag>
const EXPECTED_EPOCH_TIMESPAN: u32 = match option_env!("BITCOIN_NETWORK") {
    Some(n) if matches!(n.as_bytes(), b"signet") => 60 * 24 * 14,
    _ => 60 * 60 * 24 * 14,
};

/// Number of blocks in a difficulty adjustment epoch.
///
/// Bitcoin recalculates the difficulty target every 2016 blocks based on the time
/// it took to mine those blocks compared to the expected timespan.
const BLOCKS_PER_EPOCH: u32 = 2016;

/// Serializable representation of a Bitcoin block header.
///
/// Contains all fields from the Bitcoin block header in a format suitable for
/// zero-knowledge circuits. This struct can be serialized/deserialized and
/// converted to/from the standard `bitcoin::block::Header` type.
///
/// ## Fields
///
/// * `version` - Block version indicating which validation rules to use
/// * `prev_block_hash` - Hash of the previous block in the chain (32 bytes)
/// * `merkle_root` - Merkle tree root of all transactions in the block (32 bytes)
/// * `time` - Block timestamp as Unix time
/// * `bits` - Compact representation of the difficulty target
/// * `nonce` - Counter used in proof-of-work mining
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct CircuitBlockHeader {
    pub version: i32,
    pub prev_block_hash: [u8; 32],
    pub merkle_root: [u8; 32],
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
}

impl CircuitBlockHeader {
    /// Computes the double SHA256 hash of the block header.
    ///
    /// This implements Bitcoin's block hashing algorithm:
    /// 1. Serialize header fields in little-endian format
    /// 2. Compute SHA256 hash of the serialized data
    /// 3. Compute SHA256 hash of the result from step 2
    ///
    /// ## Returns
    ///
    /// * `[u8; 32]` - The double SHA256 hash of the block header
    pub fn compute_block_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.merkle_root);
        hasher.update(self.time.to_le_bytes());
        hasher.update(self.bits.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        let first_hash_result = hasher.finalize_reset();

        hasher.update(first_hash_result);
        let result: [u8; 32] = hasher.finalize().into();
        result
    }
}

impl From<Header> for CircuitBlockHeader {
    fn from(header: Header) -> Self {
        CircuitBlockHeader {
            version: header.version.to_consensus(),
            prev_block_hash: header.prev_blockhash.to_byte_array(),
            merkle_root: header.merkle_root.as_raw_hash().to_byte_array(),
            time: header.time,
            bits: header.bits.to_consensus(),
            nonce: header.nonce,
        }
    }
}

impl From<CircuitBlockHeader> for Header {
    fn from(val: CircuitBlockHeader) -> Self {
        Header {
            version: Version::from_consensus(val.version),
            prev_blockhash: BlockHash::from_slice(&val.prev_block_hash)
                .expect("Previous block hash is 32 bytes"),
            merkle_root: TxMerkleNode::from_slice(&val.merkle_root)
                .expect("Merkle root is 32 bytes"),
            time: val.time,
            bits: CompactTarget::from_consensus(val.bits),
            nonce: val.nonce,
        }
    }
}

/// Verifiable state of the Bitcoin header chain.
///
/// Maintains all information necessary to verify the next block in the chain,
/// including difficulty adjustment state, timestamp validation data, and an MMR
/// for efficient block hash storage and verification.
///
/// ## Fields
///
/// * `block_height` - Current height of the chain (u32::MAX for uninitialized state)
/// * `total_work` - Cumulative proof-of-work as 32-byte big-endian integer
/// * `best_block_hash` - Hash of the most recently validated block
/// * `current_target_bits` - Current difficulty target in compact representation
/// * `epoch_start_time` - Timestamp of first block in current difficulty epoch
/// * `prev_11_timestamps` - Previous 11 block timestamps for median calculation
/// * `block_hashes_mmr` - Merkle Mountain Range storing subroots
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct ChainState {
    pub block_height: u32,
    pub total_work: [u8; 32],
    pub best_block_hash: [u8; 32],
    pub current_target_bits: u32,
    pub epoch_start_time: u32,
    pub prev_11_timestamps: [u32; 11],
    pub block_hashes_mmr: MMRGuest,
}

impl Default for ChainState {
    fn default() -> Self {
        ChainState::new()
    }
}

impl ChainState {
    /// Creates a new chain state with default values.
    pub fn new() -> Self {
        ChainState {
            block_height: u32::MAX,
            total_work: [0u8; 32],
            best_block_hash: [0u8; 32],
            current_target_bits: NETWORK_CONSTANTS.max_bits,
            epoch_start_time: 0,
            prev_11_timestamps: [0u32; 11],
            block_hashes_mmr: MMRGuest::new(),
        }
    }

    /// Creates a genesis chain state.
    ///
    /// Equivalent to `new()` but with clearer semantic meaning for genesis block scenarios.
    ///
    /// ## Returns
    ///
    /// * `Self` - A new genesis `ChainState`
    pub fn genesis_state() -> Self {
        Self::new()
    }

    /// Computes a cryptographic hash of the current chain state.
    ///
    /// Creates a deterministic hash that uniquely identifies this chain state by
    /// hashing all relevant fields including block height, total work, best block hash,
    /// difficulty parameters, timestamps, and MMR state.
    ///
    /// ## Returns
    ///
    /// * `[u8; 32]` - SHA256 hash uniquely identifying this chain state
    pub fn to_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.block_height.to_le_bytes());
        hasher.update(self.total_work);
        hasher.update(self.best_block_hash);
        hasher.update(self.current_target_bits.to_le_bytes());
        hasher.update(self.epoch_start_time.to_le_bytes());
        for timestamp in self.prev_11_timestamps {
            hasher.update(timestamp.to_le_bytes());
        }
        for hash in self.block_hashes_mmr.subroots.clone() {
            hasher.update(hash);
        }
        hasher.update(self.block_hashes_mmr.size.to_le_bytes());
        hasher.finalize().into()
    }

    /// Applies a sequence of block headers to the chain state.
    ///
    /// Processes each block header in order, performing comprehensive validation
    /// and updating the chain state accordingly. This is the core validation logic
    /// that ensures Bitcoin consensus rules are followed.
    ///
    /// ## Validation Steps (per block header)
    ///
    /// 1. **Chain Continuity**: Verifies `prev_block_hash` matches current `best_block_hash`
    /// 2. **Difficulty Validation**: Ensures `bits` field matches expected difficulty
    /// 3. **Proof of Work**: Validates block hash meets the difficulty target
    /// 4. **Timestamp Validation**: Checks timestamp > median of last 11 timestamps
    /// 5. **State Updates**: Updates height, work, best hash, MMR, and timestamps
    /// 6. **Difficulty Adjustment**: Recalculates difficulty at epoch boundaries
    ///
    /// ## Network-Specific Behavior
    ///
    /// - **Regtest**: Uses minimum difficulty, no difficulty adjustments
    /// - **Testnet4**: Allows emergency difficulty reduction after 20+ minute gaps
    /// - **Others**: Standard Bitcoin difficulty adjustment rules
    ///
    /// ## Parameters
    ///
    /// * `block_headers` - Vector of block headers to process in sequence
    ///
    /// ## Panics
    ///
    /// Panics on any validation failure including invalid hashes, chain breaks,
    /// or timestamp violations.
    pub fn apply_block_headers(&mut self, block_headers: Vec<CircuitBlockHeader>) {
        let mut current_target_bytes = if IS_REGTEST {
            NETWORK_CONSTANTS.max_target.to_be_bytes()
        } else {
            bits_to_target(self.current_target_bits)
        };
        let mut current_work: U256 = U256::from_be_bytes(self.total_work);

        let mut last_block_time = if IS_TESTNET4 {
            if self.block_height == u32::MAX {
                0
            } else {
                self.prev_11_timestamps[self.block_height as usize % 11]
            }
        } else {
            0
        };

        for block_header in block_headers {
            self.block_height = self.block_height.wrapping_add(1);

            let (target_to_use, expected_bits, work_to_add) = if IS_TESTNET4 {
                if block_header.time > last_block_time + 1200 {
                    // If the block is an epoch block, then it still has to have the real target.
                    if self.block_height % BLOCKS_PER_EPOCH == 0 {
                        (
                            current_target_bytes,
                            self.current_target_bits,
                            calculate_work(&current_target_bytes),
                        )
                    }
                    // Otherwise, if the timestamp is more than 20 minutes ahead of the last block, the block is allowed to use the maximum target.
                    else {
                        (
                            NETWORK_CONSTANTS.max_target_bytes,
                            NETWORK_CONSTANTS.max_bits,
                            MINIMUM_WORK_TESTNET,
                        )
                    }
                } else {
                    (
                        current_target_bytes,
                        self.current_target_bits,
                        calculate_work(&current_target_bytes),
                    )
                }
            } else {
                (
                    current_target_bytes,
                    self.current_target_bits,
                    calculate_work(&current_target_bytes),
                )
            };

            let new_block_hash = block_header.compute_block_hash();

            assert_eq!(
                block_header.prev_block_hash, self.best_block_hash,
                "Previous block hash does not match the best block hash. Expected: {:?}, got: {:?}",
                self.best_block_hash, block_header.prev_block_hash
            );

            if IS_REGTEST {
                assert_eq!(
                    block_header.bits, NETWORK_CONSTANTS.max_bits,
                    "Bits for regtest must be equal to the maximum bits: {}. Got: {}",
                    NETWORK_CONSTANTS.max_bits, block_header.bits
                );
            } else {
                assert_eq!(
                    block_header.bits, expected_bits,
                    "Bits for the block header must match the expected bits: {}. Got: {}",
                    expected_bits, block_header.bits
                );
            }

            check_hash_valid(&new_block_hash, &target_to_use);

            if !validate_timestamp(block_header.time, self.prev_11_timestamps) {
                panic!("Timestamp is not valid, it must be greater than the median of the last 11 timestamps");
            }

            self.block_hashes_mmr.append(new_block_hash);
            self.best_block_hash = new_block_hash;
            current_work = current_work.wrapping_add(&work_to_add);

            if !IS_REGTEST && self.block_height % BLOCKS_PER_EPOCH == 0 {
                self.epoch_start_time = block_header.time;
            }

            self.prev_11_timestamps[self.block_height as usize % 11] = block_header.time;

            if IS_TESTNET4 {
                last_block_time = block_header.time;
            }

            if !IS_REGTEST && self.block_height % BLOCKS_PER_EPOCH == BLOCKS_PER_EPOCH - 1 {
                current_target_bytes = calculate_new_difficulty(
                    self.epoch_start_time,
                    block_header.time,
                    self.current_target_bits,
                );
                self.current_target_bits = target_to_bits(&current_target_bytes);
            }
        }

        self.total_work = current_work.to_be_bytes();
    }
}

/// Calculates the median of 11 timestamps.
///
/// Used for Bitcoin's median time past (MTP) rule, which requires that a block's
/// timestamp must be greater than the median of the previous 11 blocks' timestamps.
/// This prevents miners from lying about timestamps to manipulate difficulty.
///
/// ## Parameters
///
/// * `arr` - Array of exactly 11 timestamps as u32 values
///
/// ## Returns
///
/// * `u32` - The median timestamp (6th element when sorted)
fn median(arr: [u32; 11]) -> u32 {
    let mut sorted_arr = arr;
    sorted_arr.sort_unstable();
    sorted_arr[5]
}

/// Validates a block timestamp against the median time past rule.
///
/// Implements Bitcoin's median time past (MTP) validation which requires that
/// each block's timestamp must be strictly greater than the median of the
/// previous 11 blocks' timestamps. This prevents timestamp manipulation attacks.
///
/// ## Parameters
///
/// * `block_time` - The timestamp of the block being validated
/// * `prev_11_timestamps` - Array of the previous 11 block timestamps
///
/// ## Returns
///
/// * `bool` - `true` if the timestamp is valid (greater than median), `false` otherwise
fn validate_timestamp(block_time: u32, prev_11_timestamps: [u32; 11]) -> bool {
    let median_time = median(prev_11_timestamps);
    block_time > median_time
}

/// Converts compact target representation (bits) to full 32-byte target.
///
/// Bitcoin uses a compact representation for difficulty targets in block headers.
/// This function expands the 4-byte compact format into the full 32-byte target
/// that hash values are compared against.
///
/// ## Compact Target Format
///
/// The compact target uses a floating-point-like representation:
/// - Bits 24-31: Size/exponent (how many bytes the mantissa occupies)
/// - Bits 0-23: Mantissa (the significant digits)
///
/// ## Parameters
///
/// * `bits` - Compact target representation from block header
///
/// ## Returns
///
/// * `[u8; 32]` - Full 32-byte target in big-endian format
pub fn bits_to_target(bits: u32) -> [u8; 32] {
    let size = (bits >> 24) as usize;
    let mantissa = bits & 0x00ffffff;

    let target = if size <= 3 {
        U256::from(mantissa >> (8 * (3 - size)))
    } else {
        U256::from(mantissa) << (8 * (size - 3))
    };
    target.to_be_bytes()
}

/// Converts a full 32-byte target to compact representation (bits).
///
/// This is the inverse of `bits_to_target()`, converting a full 32-byte target
/// back into Bitcoin's compact 4-byte representation used in block headers.
///
/// ## Parameters
///
/// * `target` - Full 32-byte target in big-endian format
///
/// ## Returns
///
/// * `u32` - Compact target representation suitable for block headers
fn target_to_bits(target: &[u8; 32]) -> u32 {
    let target_u256 = U256::from_be_slice(target);
    let target_bits = target_u256.bits();
    let size = (263 - target_bits) / 8;
    let mut compact_target = [0u8; 4];
    compact_target[0] = 33 - size as u8;
    compact_target[1] = target[size - 1_usize];
    compact_target[2] = target[size];
    compact_target[3] = target[size + 1_usize];
    u32::from_be_bytes(compact_target)
}

/// Calculates the new difficulty target after a difficulty adjustment epoch.
///
/// Bitcoin adjusts difficulty every 2016 blocks to maintain ~10 minute block times.
/// The adjustment is based on how long the previous 2016 blocks actually took
/// compared to the expected timespan (2 weeks).
///
/// ## Algorithm
///
/// 1. Calculate actual timespan: `last_timestamp - epoch_start_time`
/// 2. Clamp timespan to [expected/4, expected*4] to limit adjustment range
/// 3. New target = old target * actual_timespan / expected_timespan
/// 4. Ensure new target doesn't exceed network maximum
///
/// ## Parameters
///
/// * `epoch_start_time` - Timestamp of the first block in the epoch
/// * `last_timestamp` - Timestamp of the last block in the epoch  
/// * `current_target` - Current difficulty target in compact format
///
/// ## Returns
///
/// * `[u8; 32]` - New difficulty target as 32-byte array
fn calculate_new_difficulty(
    epoch_start_time: u32,
    last_timestamp: u32,
    current_target: u32,
) -> [u8; 32] {
    let mut actual_timespan = last_timestamp - epoch_start_time;
    if actual_timespan < EXPECTED_EPOCH_TIMESPAN / 4 {
        actual_timespan = EXPECTED_EPOCH_TIMESPAN / 4;
    } else if actual_timespan > EXPECTED_EPOCH_TIMESPAN * 4 {
        actual_timespan = EXPECTED_EPOCH_TIMESPAN * 4;
    }

    let current_target_bytes = bits_to_target(current_target);
    let mut new_target = U256::from_be_bytes(current_target_bytes)
        .wrapping_mul(&U256::from(actual_timespan))
        .wrapping_div(&U256::from(EXPECTED_EPOCH_TIMESPAN));

    if new_target > NETWORK_CONSTANTS.max_target {
        new_target = NETWORK_CONSTANTS.max_target;
    }
    new_target.to_be_bytes()
}

/// Validates that a block hash meets the proof-of-work requirement.
///
/// Compares the block hash against the difficulty target to ensure sufficient
/// work was performed. The hash is interpreted as a big-endian 256-bit integer
/// and must be less than or equal to the target.
///
/// Bitcoin uses little-endian byte order for hashes in most contexts, but for
/// difficulty comparison the hash bytes are reversed to big-endian format.
///
/// ## Parameters
///
/// * `hash` - The block hash to validate (32 bytes, little-endian)
/// * `target_bytes` - The difficulty target (32 bytes, big-endian)
///
/// ## Panics
///
/// Panics with "Hash is not valid" if the hash exceeds the target.
fn check_hash_valid(hash: &[u8; 32], target_bytes: &[u8; 32]) {
    for i in 0..32 {
        match hash[31 - i].cmp(&target_bytes[i]) {
            Ordering::Less => return,
            Ordering::Greater => panic!("Hash is not valid"),
            Ordering::Equal => continue,
        }
    }
}

/// Calculates the amount of work represented by a difficulty target.
///
/// Bitcoin measures cumulative proof-of-work as the sum of work done by all blocks.
/// The work for a single block is inversely proportional to its target:
/// work = max_target / (target + 1)
///
/// This allows comparing the total work between different chains to determine
/// which has the most accumulated proof-of-work.
///
/// ## Parameters
///
/// * `target` - The difficulty target as a 32-byte big-endian array
///
/// ## Returns
///
/// * `U256` - The amount of work represented by this target
fn calculate_work(target: &[u8; 32]) -> U256 {
    let target = U256::from_be_slice(target);
    let target_plus_one = target.saturating_add(&U256::ONE);
    U256::MAX.wrapping_div(&target_plus_one)
}

/// Circuit output containing the updated chain state and metadata.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct BlockHeaderCircuitOutput {
    pub method_id: [u32; 8],
    pub genesis_state_hash: [u8; 32],
    pub chain_state: ChainState,
}

/// Previous proof type - either genesis state or previous circuit output.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub enum HeaderChainPrevProofType {
    GenesisBlock(ChainState),
    PrevProof(BlockHeaderCircuitOutput),
}

/// The input of the header chain circuit.
/// It contains the method ID, the previous proof (either a genesis block or a previous proof), and the block headers to be processed.
/// Method ID is used to identify the circuit and is expected to be the same as the one used in the previous proof.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct HeaderChainCircuitInput {
    pub method_id: [u32; 8],
    pub prev_proof: HeaderChainPrevProofType,
    pub block_headers: Vec<CircuitBlockHeader>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    // From block 800000 to 800015
    const BLOCK_HEADERS: [[u8; 80]; 15] = [
        hex!("00601d3455bb9fbd966b3ea2dc42d0c22722e4c0c1729fad17210100000000000000000055087fab0c8f3f89f8bcfd4df26c504d81b0a88e04907161838c0c53001af09135edbd64943805175e955e06"),
        hex!("00a0012054a02827d7a8b75601275a160279a3c5768de4c1c4a702000000000000000000394ddc6a5de035874cfa22167bfe923953187b5a19fbb84e186dea3c78fd871c9bedbd6494380517f5c93c8c"),
        hex!("00a00127470972473293f6a3514d69d9ede5acc79ef19c236be20000000000000000000035aa0cba25ae1517a257d8c913e24ec0a152fd6a84b7f9ef303626c91cdcd6b287efbd649438051761ba50fb"),
        hex!("00e0ff3fe80aaef89174e3668cde4cefecae739cd2f337251e12050000000000000000004ed17162f118bd27ae283be8dabe8afe7583bd353087e2eb712c48e3c3240c3ea3efbd64943805178e55bb2f"),
        hex!("00006020f9dd40733234ec3084fa55ae955d2e95f63db75382b4030000000000000000006f440ea93df1e46fa47a6135ce1661cbdb80e703e4cfb6d2c0bcf49ea50f2a1530f5bd64943805175d3a7efb"),
        hex!("0040f526ba869c2271583b645767c3bc4acee3f4a5a1ac727d07050000000000000000006ce5ff483f5e9fe028725bd30196a064a761b3ea831e5b81cf1473d5aa11810efbf6bd64943805174c75b45d"),
        hex!("0000c0204d770ec7842342bcfebba4447545383c639294a6c10c0500000000000000000059f61d610ef6cbcc1d05dec4ebc5e744c62dc975c4256c5f95833d350303c05521fabd64943805172b8e799e"),
        hex!("00400020d9ea5216f276b3f623834e8db837f8b41a8afbda6e8800000000000000000000d5dea9ae25f7f8e6d66064b21c7f5d1481d08d162658785fde59716b1bf98ff50505be6494380517a33ee2b0"),
        hex!("0060262ebabd5319d7013811214809650a635c974444813935b203000000000000000000a0ab544e5055c443256debb20e85f8ded28f746436a57c00e914b9fd02ff058bcf07be64943805172436ed21"),
        hex!("00000020455bd24740ceb627a3c41c3cecaf097b45779719b0d40400000000000000000043ad55fc5619dd8f2edd7d18212d176cdb6aa2152f12addf9d38c9c29be0da60030bbe649438051704743edc"),
        hex!("00e0ff27d53e9a409bf8ce3054862f76d926437c1b1a84ce1ac0010000000000000000004fceebb8a6cee0eaba389e462ae6bb89a8e6dd5396eeba89dc5907ff51112e21760dbe64943805174bd6f6f6"),
        hex!("00e0ff3ff9d1af6c7009b9974b4d838a2505bc882a6333f92500030000000000000000002dff4798432eb3beaf3e5b7c7ca318c1b451ba05c560473b6b974138ac73a82f2b0ebe6494380517d26b2853"),
        hex!("00403a31ee9197174b65726fa7d78fe8b547c024519642009b4f0100000000000000000025f09dbf49cabe174066ebc2d5329211bd994a2b645e4086cadc5a2bbe7cac687e0ebe64943805171f930c95"),
        hex!("0000eb2f06d50bd6ead9973ec74d9f5d77aa9cc6262a497b7ef5040000000000000000004918ae9062a90bfc4c2befca6eb0569c86b53f20bfae39c14d56052eef74f39e2110be64943805176269f908"),
        hex!("00a0002049b01d8eea4b9d88fabd6a9633699c579145a8ddc91205000000000000000000368d0d166ae485674d0b794a8e2e2f4e94ac1e5b6d56612b3d725bc793f523514712be6494380517860d95e4")
    ];

    const DIFFICULTY_ADJUSTMENTS: [(u32, u32, u32, u32); 430] = [
        (1231006505, 1233061996, 486604799, 486604799),
        (1233063531, 1234465122, 486604799, 486604799),
        (1234466190, 1235965467, 486604799, 486604799),
        (1235966513, 1237507400, 486604799, 486604799),
        (1237508786, 1239054978, 486604799, 486604799),
        (1239055463, 1240599092, 486604799, 486604799),
        (1240599098, 1242098000, 486604799, 486604799),
        (1242098425, 1243735798, 486604799, 486604799),
        (1243737085, 1246050840, 486604799, 486604799),
        (1246051973, 1248481522, 486604799, 486604799),
        (1248481816, 1252066931, 486604799, 486604799),
        (1252069298, 1254454291, 486604799, 486604799),
        (1254454028, 1257000207, 486604799, 486604799),
        (1257002900, 1259358448, 486604799, 486604799),
        (1259358667, 1261128623, 486604799, 486604799),
        (1261130161, 1262152739, 486604799, 486594666),
        (1262153464, 1263249842, 486594666, 486589480),
        (1263250117, 1264424481, 486589480, 486588017),
        (1264424879, 1265318937, 486588017, 486575299),
        (1265319794, 1266190073, 486575299, 476399191),
        (1266191579, 1267000203, 476399191, 474199013),
        (1267000864, 1268010273, 474199013, 473464687),
        (1268010873, 1269211443, 473464687, 473437045),
        (1269212064, 1270119474, 473437045, 472518933),
        (1270120042, 1271061370, 472518933, 471907495),
        (1271061586, 1271886653, 471907495, 471225455),
        (1271886772, 1272966003, 471225455, 471067731),
        (1272966376, 1274278387, 471067731, 471178276),
        (1274278435, 1275140649, 471178276, 470771548),
        (1275141448, 1276297992, 470771548, 470727268),
        (1276298786, 1277382263, 470727268, 470626626),
        (1277382446, 1278381204, 470626626, 470475923),
        (1278381464, 1279007808, 470475923, 470131700),
        (1279008237, 1279297671, 470131700, 469854461),
        (1279297779, 1280196974, 469854461, 469830746),
        (1280198558, 1281037393, 469830746, 469809688),
        (1281037595, 1281869965, 469809688, 469794830),
        (1281870671, 1282863700, 469794830, 459874456),
        (1282864403, 1283922146, 459874456, 459009510),
        (1283922289, 1284861793, 459009510, 457664237),
        (1284861847, 1285703762, 457664237, 456241827),
        (1285703908, 1286861405, 456241827, 456101533),
        (1286861705, 1287637343, 456101533, 454983370),
        (1287637995, 1288478771, 454983370, 454373987),
        (1288479527, 1289303926, 454373987, 453931606),
        (1289305768, 1290104845, 453931606, 453610282),
        (1290105874, 1291134100, 453610282, 453516498),
        (1291135075, 1291932610, 453516498, 453335379),
        (1291933202, 1292956393, 453335379, 453281356),
        (1292956443, 1294030806, 453281356, 453248203),
        (1294031411, 1295101259, 453248203, 453217774),
        (1295101567, 1296114735, 453217774, 453179945),
        (1296116171, 1297140342, 453179945, 453150034),
        (1297140800, 1298003311, 453150034, 453102630),
        (1298006152, 1298799509, 453102630, 453062093),
        (1298800760, 1299683275, 453062093, 453041201),
        (1299684355, 1301020485, 453041201, 453047097),
        (1301020785, 1302034036, 453047097, 453036989),
        (1302034197, 1303112797, 453036989, 453031340),
        (1303112976, 1304131540, 453031340, 453023994),
        (1304131980, 1304974694, 453023994, 443192243),
        (1304975844, 1305755857, 443192243, 440711666),
        (1305756287, 1306435280, 440711666, 438735905),
        (1306435316, 1307362613, 438735905, 438145839),
        (1307363105, 1308145551, 438145839, 437461381),
        (1308145774, 1308914894, 437461381, 437004818),
        (1308915923, 1309983257, 437004818, 436911055),
        (1309984546, 1311102675, 436911055, 436857860),
        (1311103389, 1312186259, 436857860, 436789733),
        (1312186279, 1313451537, 436789733, 436816518),
        (1313451894, 1314680496, 436816518, 436826083),
        (1314681303, 1315906303, 436826083, 436833957),
        (1315906316, 1317163240, 436833957, 436858461),
        (1317163624, 1318555415, 436858461, 436956491),
        (1318556675, 1320032359, 436956491, 437121226),
        (1320032534, 1321253256, 437121226, 437129626),
        (1321253770, 1322576247, 437129626, 437215665),
        (1322576420, 1323718660, 437215665, 437159528),
        (1323718955, 1324923455, 437159528, 437155514),
        (1324925005, 1326046766, 437155514, 437086679),
        (1326047176, 1327204081, 437086679, 437048383),
        (1327204504, 1328351050, 437048383, 437004555),
        (1328351561, 1329564101, 437004555, 437006492),
        (1329564255, 1330676346, 437006492, 436942092),
        (1330676736, 1331885274, 436942092, 436941447),
        (1331885394, 1332999614, 436941447, 436883582),
        (1332999707, 1334246594, 436883582, 436904419),
        (1334246689, 1335511874, 436904419, 436936439),
        (1335512370, 1336565211, 436936439, 436841986),
        (1336565313, 1337882969, 436841986, 436898655),
        (1337883029, 1339098664, 436898655, 436902102),
        (1339099525, 1340208670, 436902102, 436844426),
        (1340208964, 1341401376, 436844426, 436835377),
        (1341401841, 1342536951, 436835377, 436796718),
        (1342537166, 1343645636, 436796718, 436747465),
        (1343647577, 1344772046, 436747465, 436709470),
        (1344772855, 1345858666, 436709470, 436658110),
        (1345859199, 1346955024, 436658110, 436615736),
        (1346955037, 1348092805, 436615736, 436591499),
        (1348092851, 1349227021, 436591499, 436567560),
        (1349226660, 1350429295, 436567560, 436565487),
        (1350428168, 1351552830, 436565487, 436540357),
        (1351556195, 1352742671, 436540357, 436533995),
        (1352743186, 1353928117, 436533995, 436527338),
        (1353928229, 1355162497, 436527338, 436533858),
        (1355162613, 1356530758, 436533858, 436576619),
        (1356530740, 1357639870, 436576619, 436545969),
        (1357641634, 1358965635, 436545969, 436577969),
        (1358966487, 1360062830, 436577969, 436543292),
        (1360063146, 1361148326, 436543292, 436508764),
        (1361148470, 1362159549, 436508764, 436459339),
        (1362159764, 1363249652, 436459339, 436434426),
        (1363249946, 1364125673, 436434426, 436371822),
        (1364126425, 1365181981, 436371822, 436350910),
        (1365183643, 1366217849, 436350910, 436330132),
        (1366218134, 1367295455, 436330132, 436316733),
        (1367296471, 1368385955, 436316733, 436305897),
        (1368386123, 1369499565, 436305897, 436298084),
        (1369499746, 1370441773, 436298084, 436278071),
        (1370442318, 1371418407, 436278071, 436264469),
        (1371418654, 1372515090, 436264469, 436259150),
        (1372515725, 1373502151, 436259150, 436249641),
        (1373502163, 1374514657, 436249641, 436242792),
        (1374515827, 1375526943, 436242792, 426957810),
        (1375527115, 1376417294, 426957810, 424970034),
        (1376417490, 1377352245, 424970034, 423711319),
        (1377353319, 1378268176, 423711319, 422668188),
        (1378268460, 1379202097, 422668188, 421929506),
        (1379202248, 1380117691, 421929506, 421321760),
        (1380118146, 1381069174, 421321760, 420917450),
        (1381070552, 1381925718, 420917450, 420481718),
        (1381925788, 1382754194, 420481718, 420150405),
        (1382754272, 1383679776, 420150405, 419981299),
        (1383681123, 1384695132, 419981299, 419892219),
        (1384699499, 1385741656, 419892219, 419828290),
        (1385742648, 1386684666, 419828290, 419740270),
        (1386684686, 1387615098, 419740270, 419668748),
        (1387617112, 1388624139, 419668748, 419628831),
        (1388624318, 1389583107, 419628831, 419587686),
        (1389583220, 1390569911, 419587686, 419558700),
        (1390570126, 1391582444, 419558700, 419537774),
        (1391584456, 1392597647, 419537774, 419520339),
        (1392597839, 1393589930, 419520339, 419504166),
        (1393590585, 1394676535, 419504166, 419496625),
        (1394676764, 1395703577, 419496625, 419486617),
        (1395703832, 1396693489, 419486617, 419476394),
        (1396694478, 1397755194, 419476394, 419470732),
        (1397755646, 1398810754, 419470732, 419465580),
        (1398811175, 1399904296, 419465580, 410792019),
        (1399904311, 1400928544, 410792019, 409544770),
        (1400928750, 1402004511, 409544770, 408782234),
        (1402004993, 1403061308, 408782234, 408005538),
        (1403061280, 1404029522, 408005538, 406937553),
        (1404029556, 1405203024, 406937553, 406809574),
        (1405205894, 1406325104, 406809574, 406498978),
        (1406325092, 1407473800, 406498978, 406305378),
        (1407474112, 1408474964, 406305378, 405675096),
        (1408475518, 1409527066, 405675096, 405280238),
        (1409527152, 1410639387, 405280238, 405068777),
        (1410638896, 1411679882, 405068777, 404732051),
        (1411680080, 1412877894, 404732051, 404711795),
        (1412877866, 1414054419, 404711795, 404655552),
        (1414055393, 1415154489, 404655552, 404472624),
        (1415154631, 1416343330, 404472624, 404441185),
        (1416345124, 1417563570, 404441185, 404454260),
        (1417563705, 1418790160, 404454260, 404479356),
        (1418791024, 1419965406, 404479356, 404426186),
        (1419965588, 1421083565, 404426186, 404291887),
        (1421084073, 1422372768, 404291887, 404399040),
        (1422372946, 1423495952, 404399040, 404274055),
        (1423496415, 1424648263, 404274055, 404196666),
        (1424648937, 1425839583, 404196666, 404172480),
        (1425840165, 1427068149, 404172480, 404195570),
        (1427068411, 1428211256, 404195570, 404110449),
        (1428211345, 1429467587, 404110449, 404166640),
        (1429467906, 1430676673, 404166640, 404165597),
        (1430677341, 1431858092, 404165597, 404129525),
        (1431858433, 1433098989, 404129525, 404167307),
        (1433099185, 1434257600, 404167307, 404103235),
        (1434257763, 1435474473, 404103235, 404111758),
        (1435475246, 1436645194, 404111758, 404063944),
        (1436646286, 1437828076, 404063944, 404031509),
        (1437828285, 1439028210, 404031509, 404020484),
        (1439028930, 1440203823, 404020484, 403981252),
        (1440204583, 1441356822, 403981252, 403918273),
        (1441357507, 1442518636, 403918273, 403867578),
        (1442519404, 1443699609, 403867578, 403838066),
        (1443700390, 1444908588, 403838066, 403836692),
        (1444908751, 1446091729, 403836692, 403810644),
        (1446092706, 1447236281, 403810644, 403747465),
        (1447236692, 1448331948, 403747465, 403644022),
        (1448332462, 1449444509, 403644022, 403564111),
        (1449444652, 1450468554, 403564111, 403424265),
        (1450469289, 1451557421, 403424265, 403346833),
        (1451558562, 1452667067, 403346833, 403288859),
        (1452667178, 1453809473, 403288859, 403253488),
        (1453810745, 1454818212, 403253488, 403153172),
        (1454818360, 1455884612, 403153172, 403093919),
        (1455885256, 1457133524, 403093919, 403108008),
        (1457133956, 1458291885, 403108008, 403088579),
        (1458292068, 1459491849, 403088579, 403085044),
        (1459492475, 1460622012, 403085044, 403056459),
        (1460622341, 1461832072, 403056459, 403056502),
        (1461832110, 1462944601, 403056502, 403024122),
        (1462944866, 1464123775, 403024122, 403014710),
        (1464123766, 1465353421, 403014710, 403020704),
        (1465353718, 1466485981, 403020704, 402997206),
        (1466486338, 1467673575, 402997206, 402990845),
        (1467674161, 1468883232, 402990845, 402990697),
        (1468884162, 1470163257, 402990697, 403010088),
        (1470163842, 1471287293, 403010088, 402984668),
        (1471287554, 1472478633, 402984668, 402979592),
        (1472479861, 1473662270, 402979592, 402972254),
        (1473662347, 1474794756, 402972254, 402951892),
        (1474795015, 1475923695, 402951892, 402931908),
        (1475924010, 1477157004, 402931908, 402937298),
        (1477159378, 1478364220, 402937298, 402936180),
        (1478364418, 1479457348, 402936180, 402908884),
        (1479457815, 1480646474, 402908884, 402904457),
        (1480646786, 1481765173, 402904457, 402885509),
        (1481765313, 1482946227, 402885509, 402879999),
        (1482946855, 1484087479, 402879999, 402867065),
        (1484088052, 1485125083, 402867065, 402836551),
        (1485125572, 1486251490, 402836551, 402823865),
        (1486251529, 1487410067, 402823865, 402816659),
        (1487410706, 1488567833, 402816659, 402809567),
        (1488567886, 1489739512, 402809567, 402804657),
        (1489739775, 1490891447, 402804657, 402797402),
        (1490891948, 1492052381, 402797402, 402791539),
        (1492052390, 1493259291, 402791539, 402791230),
        (1493259601, 1494387130, 402791230, 402781863),
        (1494387648, 1495524275, 402781863, 402774100),
        (1495524592, 1496586576, 402774100, 402759343),
        (1496586907, 1497740528, 402759343, 402754430),
        (1497741533, 1498956326, 402754430, 402754864),
        (1498956437, 1500021909, 402754864, 402742748),
        (1500021942, 1501153235, 402742748, 402736949),
        (1501153434, 1502280491, 402736949, 402731232),
        (1502282210, 1503539571, 402731232, 402734313),
        (1503539857, 1504704167, 402734313, 402731275),
        (1504704195, 1505715737, 402731275, 402718488),
        (1505716276, 1506903856, 402718488, 402717299),
        (1506904066, 1508039962, 402717299, 402713392),
        (1508040302, 1509036725, 402713392, 402702781),
        (1509036762, 1510324761, 402702781, 402705995),
        (1510326831, 1511552082, 402705995, 402706678),
        (1511553196, 1512577362, 402706678, 402698477),
        (1512577401, 1513604778, 402698477, 402691653),
        (1513605320, 1514778580, 402691653, 402690497),
        (1514778970, 1515827472, 402690497, 394155916),
        (1515827554, 1516862792, 394155916, 392962374),
        (1516862900, 1517958218, 392962374, 392292856),
        (1517958487, 1519114710, 392292856, 392009692),
        (1519114859, 1520220349, 392009692, 391481763),
        (1520223678, 1521373214, 391481763, 391203401),
        (1521373218, 1522566103, 391203401, 391129783),
        (1522566357, 1523672538, 391129783, 390680589),
        (1523672932, 1524827574, 390680589, 390462291),
        (1524828253, 1526002294, 390462291, 390327465),
        (1526003655, 1527167457, 390327465, 390158921),
        (1527168053, 1528222495, 390158921, 389609537),
        (1528222686, 1529399698, 389609537, 389508950),
        (1529400045, 1530545107, 389508950, 389315112),
        (1530545661, 1531798474, 389315112, 389437975),
        (1531799449, 1532852342, 389437975, 388976507),
        (1532852371, 1533978695, 388976507, 388763047),
        (1533980459, 1535129301, 388763047, 388618029),
        (1535129431, 1536288716, 388618029, 388503969),
        (1536290079, 1537477114, 388503969, 388454943),
        (1537478139, 1538638684, 388454943, 388350353),
        (1538639362, 1539894787, 388350353, 388444093),
        (1539895067, 1541104406, 388444093, 388443538),
        (1541105656, 1542411813, 388443538, 388648495),
        (1542412284, 1543837587, 388648495, 389142908),
        (1543838368, 1545175878, 389142908, 389488372),
        (1545175965, 1546275302, 389488372, 389159077),
        (1546276809, 1547431851, 389159077, 389010995),
        (1547432394, 1548656416, 389010995, 389048373),
        (1548657313, 1549817652, 389048373, 388919176),
        (1549817981, 1551025524, 388919176, 388914000),
        (1551026038, 1552236227, 388914000, 388915479),
        (1552236304, 1553387053, 388915479, 388767596),
        (1553387093, 1554594090, 388767596, 388761373),
        (1554594223, 1555811438, 388761373, 388779537),
        (1555811668, 1556958256, 388779537, 388628280),
        (1556958733, 1558167889, 388628280, 388627269),
        (1558168296, 1559255464, 388627269, 388348790),
        (1559256184, 1560473993, 388348790, 388365571),
        (1560474230, 1561603749, 388365571, 388200748),
        (1561604370, 1562663247, 388200748, 387911067),
        (1562663868, 1563880228, 387911067, 387922440),
        (1563880937, 1564972845, 387922440, 387723321),
        (1564973528, 1566159593, 387723321, 387687377),
        (1566161382, 1567304898, 387687377, 387588414),
        (1567305301, 1568401109, 387588414, 387427317),
        (1568401591, 1569528791, 387427317, 387321636),
        (1569530001, 1570716515, 387321636, 387294044),
        (1570716535, 1571865760, 387294044, 387223263),
        (1571866973, 1573168955, 387223263, 387326161),
        (1573169436, 1574355426, 387326161, 387297854),
        (1574356132, 1575574787, 387297854, 387308498),
        (1575576145, 1576779043, 387308498, 387300560),
        (1576779421, 1577914494, 387300560, 387212786),
        (1577915667, 1579045242, 387212786, 387124344),
        (1579045357, 1580201014, 387124344, 387068671),
        (1580201043, 1581404369, 387068671, 387062484),
        (1581405024, 1582619298, 387062484, 387067068),
        (1582619322, 1583751024, 387067068, 386990361),
        (1583751917, 1585191082, 386990361, 387201857),
        (1585191106, 1586334725, 387201857, 387129532),
        (1586336046, 1587451399, 387129532, 387031859),
        (1587452724, 1588651347, 387031859, 387021369),
        (1588651521, 1589938370, 387021369, 387094518),
        (1589940416, 1591273835, 387094518, 387219253),
        (1591273852, 1592326176, 387219253, 387044594),
        (1592326267, 1593535908, 387044594, 387044633),
        (1593537529, 1594638224, 387044633, 386939413),
        (1594641060, 1595886443, 386939413, 386970872),
        (1595886756, 1597089202, 386970872, 386964396),
        (1597089619, 1598257182, 386964396, 386926570),
        (1598258059, 1599482443, 386926570, 386939410),
        (1599482920, 1600569231, 386939410, 386831018),
        (1600570533, 1601781172, 386831018, 386831838),
        (1601781592, 1602948896, 386831838, 386798414),
        (1602950620, 1604391477, 386798414, 386974771),
        (1604392090, 1605546079, 386974771, 386924253),
        (1605546119, 1606657197, 386924253, 386838870),
        (1606657305, 1607898457, 386838870, 386863986),
        (1607899483, 1609113673, 386863986, 386867735),
        (1609113744, 1610205491, 386867735, 386771105),
        (1610205877, 1611402924, 386771105, 386761815),
        (1611403017, 1612578145, 386761815, 386736569),
        (1612578303, 1613771771, 386736569, 386725091),
        (1613772036, 1614997194, 386725091, 386736012),
        (1614997708, 1616184225, 386736012, 386719599),
        (1616184405, 1617327513, 386719599, 386673224),
        (1617328801, 1618515600, 386673224, 386658195),
        (1618515703, 1619899807, 386658195, 386771043),
        (1619900822, 1620896111, 386771043, 386612457),
        (1620896338, 1622335745, 386612457, 386752379),
        (1622337521, 1623614781, 386752379, 386801401),
        (1623614836, 1625293501, 386801401, 387160270),
        (1625294046, 1626564728, 387160270, 387225124),
        (1626564737, 1627705595, 387225124, 387148450),
        (1627706126, 1628833331, 387148450, 387061771),
        (1628834027, 1629902243, 387061771, 386923168),
        (1629902476, 1631059521, 386923168, 386877668),
        (1631061045, 1632233558, 386877668, 386846955),
        (1632234876, 1633390031, 386846955, 386803250),
        (1633390519, 1634588711, 386803250, 386794504),
        (1634588757, 1635710294, 386794504, 386727631),
        (1635710370, 1636865834, 386727631, 386689514),
        (1636866927, 1638094859, 386689514, 386701843),
        (1638095408, 1639212040, 386701843, 386638367),
        (1639216857, 1640422619, 386638367, 386635947),
        (1640422999, 1641627659, 386635947, 386632843),
        (1641627937, 1642734420, 386632843, 386568320),
        (1642734490, 1643941946, 386568320, 386567092),
        (1643942057, 1645096442, 386567092, 386535544),
        (1645096491, 1646324392, 386535544, 386545523),
        (1646324511, 1647538413, 386545523, 386547904),
        (1647538808, 1648700407, 386547904, 386521239),
        (1648700729, 1649925810, 386521239, 386529497),
        (1649925939, 1651071862, 386529497, 386495093),
        (1651072835, 1652226053, 386495093, 386466234),
        (1652226078, 1653490447, 386466234, 386492960),
        (1653490985, 1654685173, 386492960, 386485098),
        (1654686448, 1655925220, 386485098, 386499788),
        (1655925489, 1657152407, 386499788, 386508719),
        (1657153358, 1658426742, 386508719, 386542084),
        (1658427282, 1659616186, 386542084, 386530686),
        (1659617683, 1660819735, 386530686, 386526600),
        (1660820877, 1661927959, 386526600, 386471456),
        (1661928055, 1663097332, 386471456, 386451604),
        (1663097346, 1664333361, 386451604, 386464174),
        (1664333794, 1665399026, 386464174, 386393970),
        (1665399506, 1666568884, 386393970, 386376745),
        (1666569091, 1667781109, 386376745, 386377746),
        (1667781163, 1668984601, 386377746, 386375189),
        (1668986059, 1670291250, 386375189, 386414640),
        (1670291429, 1671462730, 386414640, 386397584),
        (1671463076, 1672717752, 386397584, 386417022),
        (1672719770, 1673816848, 386417022, 386366690),
        (1673817110, 1674972595, 386366690, 386344736),
        (1674972641, 1676188253, 386344736, 386347065),
        (1676188371, 1677288474, 386347065, 386304419),
        (1677288852, 1678484625, 386304419, 386299521),
        (1678484890, 1679609492, 386299521, 386269758),
        (1679609802, 1680793023, 386269758, 386261170),
        (1680795199, 1681984324, 386261170, 386254649),
        (1681984653, 1683212066, 386254649, 386260225),
        (1683214087, 1684385994, 386260225, 386248250),
        (1684386462, 1685556292, 386248250, 386236009),
        (1685557167, 1686740979, 386236009, 386228333),
        (1686742062, 1687992366, 386228333, 386240190),
        (1687992515, 1689128861, 386240190, 386218132),
        (1689128979, 1690375168, 386218132, 386228482),
        (1690375347, 1691583496, 386228482, 386228059),
        (1691584068, 1692723421, 386228059, 386207611),
        (1692724599, 1693967068, 386207611, 386216622),
        (1693967242, 1695113955, 386216622, 386198911),
        (1695114421, 1696319769, 386198911, 386197775),
        (1696319920, 1697456008, 386197775, 386178217),
        (1697455965, 1698637823, 386178217, 386171284),
        (1698638003, 1699806178, 386171284, 386161170),
        (1699806273, 1700957506, 386161170, 386147408),
        (1700957763, 1702179081, 386147408, 386150037),
        (1702180644, 1703311291, 386150037, 386132147),
        (1703311464, 1704501378, 386132147, 386127977),
        (1704501692, 1705760375, 386127977, 386138202),
        (1705761155, 1706888111, 386138202, 386120285),
        (1706888526, 1708006020, 386120285, 386101681),
        (1708008110, 1709253900, 386101681, 386108434),
        (1709253937, 1710397305, 386108434, 386095705),
        (1710397689, 1711619239, 386095705, 386097875),
        (1711619463, 1712783397, 386097875, 386089497),
        (1712783853, 1713969900, 386089497, 386085339),
        (1713970312, 1715252012, 386085339, 386097818),
        (1715252414, 1716444342, 386097818, 386094576),
        (1716445130, 1717664336, 386094576, 386096312),
        (1717664663, 1718874866, 386096312, 386096421),
        (1718875797, 1720149002, 386096421, 386108013),
        (1720149673, 1721321644, 386108013, 386100794),
        (1721322584, 1722417204, 386100794, 386079422),
        (1722417212, 1723679655, 386079422, 386088310),
        (1723679961, 1724854413, 386088310, 386082139),
        (1724855515, 1726023352, 386082139, 386075020),
        (1726025157, 1727293148, 386075020, 386084628),
        (1727293228, 1728454931, 386084628, 386076365),
        (1728456399, 1729620194, 386076365, 386068776),
    ];

    #[test]
    fn test_block_hash_calculation() {
        let merkle_root = hex!("3ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a");
        let expected_block_hash =
            hex!("6fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000");

        let block_header = CircuitBlockHeader {
            version: 1,
            prev_block_hash: [0u8; 32],
            merkle_root,
            time: 1231006505,
            bits: 486604799,
            nonce: 2083236893,
        };

        let block_hash = block_header.compute_block_hash();
        assert_eq!(block_hash, expected_block_hash);
    }

    #[test]
    fn test_15_block_hash_calculation() {
        let block_headers = BLOCK_HEADERS
            .iter()
            .map(|header| CircuitBlockHeader::try_from_slice(header).unwrap())
            .collect::<Vec<CircuitBlockHeader>>();

        for i in 0..block_headers.len() - 1 {
            let block_hash = block_headers[i].compute_block_hash();
            let next_block = &block_headers[i + 1];
            assert_eq!(block_hash, next_block.prev_block_hash);
        }
    }

    #[test]
    fn test_median() {
        let arr = [3, 7, 2, 10, 1, 5, 9, 4, 8, 6, 11];
        assert_eq!(median(arr), 6);
    }

    #[test]
    fn test_timestamp_check_fail() {
        let block_headers = BLOCK_HEADERS
            .iter()
            .map(|header| CircuitBlockHeader::try_from_slice(header).unwrap())
            .collect::<Vec<CircuitBlockHeader>>();

        let first_11_timestamps = block_headers[..11]
            .iter()
            .map(|header| header.time)
            .collect::<Vec<u32>>();

        // The validation is expected to return false
        assert!(!validate_timestamp(
            block_headers[1].time,
            first_11_timestamps.try_into().unwrap(),
        ));
    }

    #[test]
    fn test_timestamp_check_pass() {
        let block_headers = BLOCK_HEADERS
            .iter()
            .map(|header| CircuitBlockHeader::try_from_slice(header).unwrap())
            .collect::<Vec<CircuitBlockHeader>>();

        let first_11_timestamps = block_headers[..11]
            .iter()
            .map(|header| header.time)
            .collect::<Vec<u32>>();

        assert!(validate_timestamp(
            block_headers[11].time,
            first_11_timestamps.clone().try_into().unwrap(),
        ));
    }

    #[test]
    #[should_panic(expected = "Hash is not valid")]
    fn test_hash_check_fail() {
        let block_headers = BLOCK_HEADERS
            .iter()
            .map(|header| CircuitBlockHeader::try_from_slice(header).unwrap())
            .collect::<Vec<CircuitBlockHeader>>();

        let first_15_hashes = block_headers[..15]
            .iter()
            .map(|header| header.compute_block_hash())
            .collect::<Vec<[u8; 32]>>();

        // The validation is expected to panic
        check_hash_valid(
            &first_15_hashes[0],
            &U256::from_be_hex("00000000FFFF0000000000000000000000000000000000000000000000000000")
                .wrapping_div(&(U256::ONE << 157))
                .to_be_bytes(),
        );
    }

    #[test]
    fn test_hash_check_pass() {
        let block_headers = BLOCK_HEADERS
            .iter()
            .map(|header| CircuitBlockHeader::try_from_slice(header).unwrap())
            .collect::<Vec<CircuitBlockHeader>>();

        let first_15_hashes = block_headers[..15]
            .iter()
            .map(|header| header.compute_block_hash())
            .collect::<Vec<[u8; 32]>>();

        for (i, hash) in first_15_hashes.into_iter().enumerate() {
            check_hash_valid(&hash, &bits_to_target(block_headers[i].bits));
        }
    }

    #[test]
    fn test_target_conversion() {
        for (_, _, bits, _) in DIFFICULTY_ADJUSTMENTS {
            let compact_target = bits_to_target(bits);
            let nbits = target_to_bits(&compact_target);
            assert_eq!(nbits, bits);
        }
    }

    #[test]
    fn test_bits_to_target() {
        // https://learnmeabitcoin.com/explorer/block/00000000000000000002ebe388cb8fa0683fc34984cfc2d7d3b3f99bc0d51bfd
        let expected_target =
            hex!("00000000000000000002f1280000000000000000000000000000000000000000");
        let bits: u32 = 0x1702f128;
        let target = bits_to_target(bits);
        assert_eq!(target, expected_target);

        let converted_bits = target_to_bits(&target);

        assert_eq!(converted_bits, bits);
    }

    #[test]
    fn test_difficulty_adjustments() {
        for (start_time, end_time, start_target, end_target) in DIFFICULTY_ADJUSTMENTS {
            let new_target_bytes = calculate_new_difficulty(start_time, end_time, start_target);
            let bits = target_to_bits(&new_target_bytes);
            assert_eq!(bits, end_target);
        }
    }

    #[test]
    fn test_bridge_block_header_from_header() {
        let header = Header {
            version: Version::from_consensus(1),
            prev_blockhash: BlockHash::from_slice(&[0; 32]).unwrap(),
            merkle_root: TxMerkleNode::from_slice(&[1; 32]).unwrap(),
            time: 1231006505,
            bits: CompactTarget::from_consensus(0x1d00ffff),
            nonce: 2083236893,
        };

        let bridge_header: CircuitBlockHeader = header.into();

        assert_eq!(bridge_header.version, header.version.to_consensus());
        assert_eq!(
            bridge_header.prev_block_hash,
            *header.prev_blockhash.as_byte_array()
        );
        assert_eq!(
            bridge_header.merkle_root,
            *header.merkle_root.as_byte_array()
        );
        assert_eq!(bridge_header.time, header.time);
        assert_eq!(bridge_header.bits, header.bits.to_consensus());
        assert_eq!(bridge_header.nonce, header.nonce);
        assert_eq!(
            bridge_header.compute_block_hash(),
            header.block_hash().to_byte_array()
        );
    }

    #[test]
    fn test_bridge_block_header_into_header() {
        let bridge_header = CircuitBlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [1; 32],
            time: 1231006505,
            bits: 0x1d00ffff,
            nonce: 2083236893,
        };

        let header: Header = bridge_header.clone().into();

        assert_eq!(header.version.to_consensus(), bridge_header.version);
        assert_eq!(
            *header.prev_blockhash.as_byte_array(),
            bridge_header.prev_block_hash
        );
        assert_eq!(
            *header.merkle_root.as_byte_array(),
            bridge_header.merkle_root
        );
        assert_eq!(header.time, bridge_header.time);
        assert_eq!(header.bits.to_consensus(), bridge_header.bits);
        assert_eq!(header.nonce, bridge_header.nonce);
        assert_eq!(
            header.block_hash().to_byte_array(),
            bridge_header.compute_block_hash()
        );
    }

    #[test]
    fn test_roundtrip_header_conversion() {
        let original_header = Header {
            version: Version::from_consensus(1),
            prev_blockhash: BlockHash::from_slice(&[0; 32]).unwrap(),
            merkle_root: TxMerkleNode::from_slice(&[1; 32]).unwrap(),
            time: 1231006505,
            bits: CompactTarget::from_consensus(0x1d00ffff),
            nonce: 2083236893,
        };

        let bridge_header: CircuitBlockHeader = original_header.into();
        let converted_header: Header = bridge_header.into();

        assert_eq!(original_header, converted_header);
        assert_eq!(original_header.block_hash(), converted_header.block_hash());
    }
}
