use crate::builder::transaction::TxHandler;
use crate::cli::Args;
use crate::config::BridgeConfig;
use crate::errors::BridgeError;
use bitcoin::taproot::{self, LeafVersion};
use bitcoin::XOnlyPublicKey;
use bitcoin::{self, Witness};
use tracing::Level;
//use bitvm::chunker::assigner::BridgeAssigner;
use std::borrow::BorrowMut;
use std::collections::BTreeMap;
use std::process::exit;
use std::str::FromStr;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

lazy_static::lazy_static! {
    /// Global secp context.
    pub static ref SECP: bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All> = bitcoin::secp256k1::Secp256k1::new();
}

lazy_static::lazy_static! {
    /// This is an unspendable pubkey.
    ///
    /// See https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki#constructing-and-spending-taproot-outputs
    pub static ref UNSPENDABLE_PUBKEY: bitcoin::secp256k1::PublicKey =
        "93c7378d96518a75448821c4f7c8f4bae7ce60f804d03d1f0628dd5dd0f5de51".parse().unwrap();
}

lazy_static::lazy_static! {
    /// This is an unspendable pubkey.
    ///
    /// See https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki#constructing-and-spending-taproot-outputs
    pub static ref UNSPENDABLE_XONLY_PUBKEY: bitcoin::secp256k1::XOnlyPublicKey =
        XOnlyPublicKey::from_str("93c7378d96518a75448821c4f7c8f4bae7ce60f804d03d1f0628dd5dd0f5de51").unwrap();
}

lazy_static::lazy_static! {
    pub static ref NETWORK : bitcoin::Network = bitcoin::Network::Regtest;
}

// lazy_static::lazy_static! {
//     pub static ref ALL_BITVM_INTERMEDIATE_VARIABLES: BTreeMap<String, usize> = BridgeAssigner::default().all_intermediate_variable();
// }

lazy_static::lazy_static! {
    pub static ref ALL_BITVM_INTERMEDIATE_VARIABLES: BTreeMap<String, usize> = {
        let mut map = BTreeMap::new();
        map.insert("scalar_1".to_string(), 20);
        map.insert("scalar_2".to_string(), 20);
        map.insert("scalar_3".to_string(), 20);
        map.insert("scalar_4".to_string(), 20);
        map.insert("scalar_5".to_string(), 20);
        map.insert("scalar_6".to_string(), 20);
        map.insert("scalar_7".to_string(), 20);
        map.insert("scalar_8".to_string(), 20);
        map.insert("scalar_9".to_string(), 20);
        map.insert("scalar_10".to_string(), 20);
        map
    };
}

/// Gets configuration from CLI, for binaries. If there are any errors, print
/// error to stderr and exit program.
///
/// Steps:
///
/// 1. Get CLI arguments
/// 2. Initialize logger
/// 3. Get configuration file
///
/// These steps are pretty standard and binaries can use this to get a
/// `BridgeConfig`.
///
/// # Returns
///
/// A tuple, containing:
///
/// - [`BridgeConfig`] from CLI argument
/// - [`Args`] from CLI options
pub fn get_configuration_for_binaries() -> (BridgeConfig, Args) {
    let args = match crate::cli::parse() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };

    let level_filter = match args.verbose {
        0 => None,
        other => Some(LevelFilter::from_level(
            Level::from_str(&other.to_string()).unwrap(),
        )),
    };

    match crate::utils::initialize_logger(level_filter) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };
    let config = match crate::cli::get_configuration_from(args.clone()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };

    (config, args)
}

pub fn usize_to_var_len_bytes(x: usize) -> Vec<u8> {
    let usize_bytes = (usize::BITS / 8) as usize;
    let bits = x.max(1).ilog2() + 1;
    let len = ((bits + 7) / 8) as usize;
    let empty = usize_bytes - len;
    let op_idx_bytes = x.to_be_bytes();
    let op_idx_bytes = &op_idx_bytes[empty..];
    op_idx_bytes.to_vec()
}

/// Constructs the witness for a script path spend of a transaction input.
///
/// # Arguments
///
/// - `tx`: The transaction to add the witness to.
/// - `script_inputs`: The inputs to the tapscript
/// - `txin_index`: The index of the transaction input to add the witness to.
/// - `script_index`: The script index in the input UTXO's Taproot script tree. This is used to get the control block and script contents of the script being spent.
pub fn set_p2tr_script_spend_witness<T: AsRef<[u8]>>(
    tx: &mut TxHandler,
    script_inputs: &[T],
    txin_index: usize,
    script_index: usize,
) -> Result<(), BridgeError> {
    let witness = tx
        .tx
        .input
        .get_mut(txin_index)
        .map(|input| &mut input.witness)
        .ok_or(BridgeError::TxInputNotFound)?;

    witness.clear();
    script_inputs
        .iter()
        .for_each(|element| witness.push(element));

    let script = &tx.prev_scripts[txin_index][script_index];
    let spend_control_block = tx.prev_taproot_spend_infos[txin_index]
        .clone()
        .ok_or(BridgeError::TaprootScriptError)?
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .ok_or(BridgeError::ControlBlockError)?;

    witness.push(script.clone());
    witness.push(spend_control_block.serialize());
    Ok(())
}

pub fn set_p2tr_key_spend_witness(
    tx: &mut TxHandler,
    signature: &taproot::Signature,
    txin_index: usize,
) -> Result<(), BridgeError> {
    let witness = tx
        .tx
        .borrow_mut()
        .input
        .get_mut(txin_index)
        .map(|input| &mut input.witness)
        .ok_or(BridgeError::TxInputNotFound)?;

    *witness = Witness::p2tr_key_spend(signature);
    Ok(())
}

/// Initializes `tracing` as the logger.
///
/// # Parameters
///
/// - `level`: Level ranges from 0 to 5. 0 defaults to no logs but can be
///   overwritten with `RUST_LOG` env var. While other numbers sets log level from
///   lowest level (1) to highest level (5). Is is advised to use 0 on tests and
///   other values for binaries (get value from user).
///
/// # Returns
///
/// Returns `Err` if `tracing` can't be initialized. Multiple subscription error
/// is emmitted and will return `Ok(())`.
pub fn initialize_logger(level: Option<LevelFilter>) -> Result<(), BridgeError> {
    // Standard layer that will output human readable logs.
    let layer = fmt::layer().with_test_writer();
    // JSON layer that will output JSON formatted logs.
    let json_layer = fmt::layer::<Registry>().with_test_writer().json();

    let filter = match level {
        Some(level) => EnvFilter::builder()
            .with_default_directive(level.into())
            .from_env_lossy(),
        None => EnvFilter::from_default_env(),
    };

    // Try to initialize tracing, depending on the `JSON_LOGS` env var,
    let res = if std::env::var("JSON_LOGS").is_ok() {
        tracing_subscriber::util::SubscriberInitExt::try_init(
            tracing_subscriber::registry().with(json_layer).with(filter),
        )
    } else {
        tracing_subscriber::util::SubscriberInitExt::try_init(
            tracing_subscriber::registry().with(layer).with(filter),
        )
    };

    if let Err(e) = res {
        // If it failed because of a re-initialization, do not care about
        // the error.
        if e.to_string() != "a global default trace dispatcher has already been set" {
            return Err(BridgeError::ConfigError(e.to_string()));
        }

        tracing::trace!("Tracing is already initialized, skipping without errors...");
    };

    Ok(())
}
