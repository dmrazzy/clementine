use bitcoin::{Amount, Network};
use serde::{Deserialize, Serialize};

const BLOCKS_PER_WEEK: u16 = 6 * 24 * 7;

const BLOCKS_PER_DAY: u16 = 6 * 24;

/// This is the log_d used across the codebase.
///
/// All protocol paramsets should use this value since it's used in the BitVM static.
pub const WINTERNITZ_LOG_D: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// A pre-defined paramset name that can be converted into a
/// [`ProtocolParamset`] reference. Refers to a defined constant paramset in this module.
///
/// See: [`MAINNET_PARAMSET`], [`REGTEST_PARAMSET`], [`TESTNET_PARAMSET`].
pub enum ProtocolParamsetName {
    Mainnet,
    Regtest,
    Testnet4,
}

impl From<ProtocolParamsetName> for &'static ProtocolParamset {
    fn from(name: ProtocolParamsetName) -> Self {
        match name {
            ProtocolParamsetName::Mainnet => &MAINNET_PARAMSET,
            ProtocolParamsetName::Regtest => &REGTEST_PARAMSET,
            ProtocolParamsetName::Testnet4 => &TESTNET4_PARAMSET,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Protocol parameters that affect the transactions in the contract (which also
/// change the pre-calculated txids and sighashes).
///
/// These parameters are used when generating the transactions and changing them
/// will break compatibility between actors, making deposits impossible.  A
/// paramset is chosen by the actor by choosing a ParamsetName inside the
/// [`crate::config::BridgeConfig`].
pub struct ProtocolParamset {
    /// Bitcoin network to work on (mainnet, testnet, regtest).
    pub network: Network,
    /// Number of round transactions that the operator will create.
    pub num_round_txs: usize,
    /// Number of kickoff UTXOs per round transaction.
    pub num_kickoffs_per_round: usize,
    /// Bridge deposit amount that users can deposit.
    pub bridge_amount: Amount,
    /// Number of watchtowers. (changes the number of watchtower challenge kickoff txouts)
    pub num_watchtowers: usize,
    /// Amount allocated for each kickoff UTXO.
    pub kickoff_amount: Amount,
    /// Amount allocated for operator challenge transactions.
    pub operator_challenge_amount: Amount,
    /// Collateral funding amount for operators used to fund the round transaction chain.
    pub collateral_funding_amount: Amount,
    /// Length of the blockhash commitment in kickoff transactions.
    pub kickoff_blockhash_commit_length: u32,
    /// Length of the message used in watchtower challenge transactions.
    pub watchtower_challenge_message_length: usize,
    /// Winternitz derivation log_d (shared for all WOTS commitments)
    ///
    /// Currently used in statics and thus cannot be different from [`WINTERNITZ_LOG_D`].
    pub winternitz_log_d: u32,
    /// Number of blocks after which user can take deposit back if deposit request fails.
    pub user_takes_after: u16,
    /// Number of blocks for operator challenge timeout timelock (currently BLOCKS_PER_WEEK)
    pub operator_challenge_timeout_timelock: u16,
    /// Number of blocks for operator challenge NACK timelock (currently BLOCKS_PER_WEEK * 3)
    pub operator_challenge_nack_timelock: u16,
    /// Number of blocks for disprove timeout timelock (currently BLOCKS_PER_WEEK * 5)
    pub disprove_timeout_timelock: u16,
    /// Number of blocks for assert timeout timelock (currently BLOCKS_PER_WEEK * 4)
    pub assert_timeout_timelock: u16,
    /// Number of blocks for operator reimburse timelock (currently BLOCKS_PER_DAY * 2)
    ///
    /// Timelocks operator from sending the next Round Tx after the Ready to Reimburse Tx.
    pub operator_reimburse_timelock: u16,
    /// Number of blocks for watchtower challenge timeout timelock (currently BLOCKS_PER_WEEK * 2)
    pub watchtower_challenge_timeout_timelock: u16,
}

pub const MAINNET_PARAMSET: ProtocolParamset = ProtocolParamset {
    network: Network::Bitcoin,
    num_round_txs: 2,
    num_kickoffs_per_round: 200,
    bridge_amount: Amount::from_sat(1_000_000_000),
    kickoff_amount: Amount::from_sat(40_000),
    operator_challenge_amount: Amount::from_sat(200_000_000),
    collateral_funding_amount: Amount::from_sat(200_000_000),
    kickoff_blockhash_commit_length: 40,
    watchtower_challenge_message_length: 480,
    winternitz_log_d: WINTERNITZ_LOG_D,
    num_watchtowers: 4,
    user_takes_after: 200,
    operator_challenge_timeout_timelock: BLOCKS_PER_WEEK,
    operator_challenge_nack_timelock: BLOCKS_PER_WEEK * 3,
    disprove_timeout_timelock: BLOCKS_PER_WEEK * 5,
    assert_timeout_timelock: BLOCKS_PER_WEEK * 4,
    operator_reimburse_timelock: BLOCKS_PER_DAY * 2,
    watchtower_challenge_timeout_timelock: BLOCKS_PER_WEEK * 2,
};

pub const REGTEST_PARAMSET: ProtocolParamset = ProtocolParamset {
    network: Network::Regtest,
    num_round_txs: 2,
    num_kickoffs_per_round: 2,
    bridge_amount: Amount::from_sat(1_000_000_000),
    kickoff_amount: Amount::from_sat(40_000),
    operator_challenge_amount: Amount::from_sat(200_000_000),
    collateral_funding_amount: Amount::from_sat(200_000_000),
    kickoff_blockhash_commit_length: 40,
    watchtower_challenge_message_length: 480,
    winternitz_log_d: WINTERNITZ_LOG_D,
    num_watchtowers: 4,
    user_takes_after: 200,
    operator_challenge_timeout_timelock: BLOCKS_PER_WEEK,
    operator_challenge_nack_timelock: BLOCKS_PER_WEEK * 3,
    disprove_timeout_timelock: BLOCKS_PER_WEEK * 5,
    assert_timeout_timelock: BLOCKS_PER_WEEK * 4,
    operator_reimburse_timelock: BLOCKS_PER_DAY * 2,
    watchtower_challenge_timeout_timelock: BLOCKS_PER_WEEK * 2,
};

pub const TESTNET4_PARAMSET: ProtocolParamset = ProtocolParamset {
    network: Network::Testnet4,
    num_round_txs: 2,
    num_kickoffs_per_round: 2,
    bridge_amount: Amount::from_sat(10_000_000),
    kickoff_amount: Amount::from_sat(40_000),
    operator_challenge_amount: Amount::from_sat(200_000_000),
    collateral_funding_amount: Amount::from_sat(200_000_000),
    kickoff_blockhash_commit_length: 40,
    watchtower_challenge_message_length: 480,
    winternitz_log_d: WINTERNITZ_LOG_D,
    num_watchtowers: 4,
    user_takes_after: 200,
    operator_challenge_timeout_timelock: BLOCKS_PER_WEEK,
    operator_challenge_nack_timelock: BLOCKS_PER_WEEK * 3,
    disprove_timeout_timelock: BLOCKS_PER_WEEK * 5,
    assert_timeout_timelock: BLOCKS_PER_WEEK * 4,
    operator_reimburse_timelock: BLOCKS_PER_DAY * 2,
    watchtower_challenge_timeout_timelock: BLOCKS_PER_WEEK * 2,
};
