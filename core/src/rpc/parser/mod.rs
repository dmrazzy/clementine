use super::clementine::{
    self, AssertRequest, Outpoint, SchnorrSig, TransactionRequest, WinternitzPubkey,
};
use super::error;
use crate::builder::transaction::sign::{AssertRequestData, TransactionRequestData};
use crate::builder::transaction::{DepositData, TransactionType};
use crate::errors::BridgeError;
use crate::EVMAddress;
use bitcoin::hashes::{sha256d, FromSliceError, Hash};
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::{OutPoint, Txid, XOnlyPublicKey};
use bitvm::signatures::winternitz;
use eyre::Context;
use std::fmt::{Debug, Display};
use std::num::TryFromIntError;
use tonic::Status;

pub mod operator;
pub mod verifier;
pub mod watchtower;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ParserError {
    // RPC errors
    #[error("RPC function field {0} is required")]
    RPCRequiredParam(&'static str),
    #[error("RPC function parameter {0} is malformed")]
    RPCParamMalformed(String),
}

impl From<ParserError> for tonic::Status {
    fn from(value: ParserError) -> Self {
        match value {
            ParserError::RPCRequiredParam(field) => {
                Status::invalid_argument(format!("RPC function field {} is required.", field))
            }
            ParserError::RPCParamMalformed(field) => {
                Status::invalid_argument(format!("RPC function parameter {} is malformed.", field))
            }
        }
    }
}

/// Converts an integer type in to another integer type. This is needed because
/// tonic defaults to wrong integer types for some parameters.
pub fn convert_int_to_another<SOURCE, TARGET>(
    field_name: &str,
    value: SOURCE,
    try_from: fn(SOURCE) -> Result<TARGET, TryFromIntError>,
) -> Result<TARGET, Status>
where
    SOURCE: Copy + Debug + Display,
{
    try_from(value)
        .map_err(|e| error::invalid_argument(field_name, "Given number is out of bounds")(e))
}

/// Fetches the next message from a stream which is unwrapped and encapsulated
/// by a [`Result`].
///
/// # Parameters
///
/// - stream: [`tonic::Streaming`] typed input stream
/// - field: Input field ident (struct member) to look in the next message
///
/// # Returns
///
/// A [`Result`] containing the next message. Will return an [`Err`] variant if
/// stream has exhausted.
#[macro_export]
macro_rules! fetch_next_message_from_stream {
    ($stream:expr, $field:ident) => {
        $crate::fetch_next_optional_message_from_stream!($stream, $field).ok_or(
            $crate::rpc::error::expected_msg_got_none(stringify!($field))(),
        )
    };
}

/// Fetches next message from a stream.
///
/// # Parameters
///
/// - stream: [`tonic::Streaming`] typed input stream
/// - field: Input field ident (struct member) to look in the next message
///
/// # Returns
///
/// An [`Option`] containing the next message. Will return a [`None`] variant if
/// stream has exhausted.
#[macro_export]
macro_rules! fetch_next_optional_message_from_stream {
    ($stream:expr, $field:ident) => {
        $stream
            .message()
            .await?
            .ok_or($crate::rpc::error::input_ended_prematurely())?
            .$field
    };
}

impl TryFrom<Outpoint> for OutPoint {
    type Error = BridgeError;

    fn try_from(value: Outpoint) -> Result<Self, Self::Error> {
        let hash = match Hash::from_slice(&value.txid) {
            Ok(h) => h,
            Err(e) => return Err(BridgeError::FromSliceError(e)),
        };

        Ok(OutPoint {
            txid: Txid::from_raw_hash(hash),
            vout: value.vout,
        })
    }
}
impl From<OutPoint> for Outpoint {
    fn from(value: OutPoint) -> Self {
        Outpoint {
            txid: value.txid.to_byte_array().to_vec(),
            vout: value.vout,
        }
    }
}

impl TryFrom<WinternitzPubkey> for winternitz::PublicKey {
    type Error = BridgeError;

    fn try_from(value: WinternitzPubkey) -> Result<Self, Self::Error> {
        let inner = value.digit_pubkey;

        inner
            .into_iter()
            .enumerate()
            .map(|(i, inner_vec)| {
                inner_vec
                    .try_into()
                    .map_err(|e: Vec<_>| eyre::eyre!("Incorrect length {:?}, expected 20", e.len()))
                    .wrap_err_with(|| {
                        ParserError::RPCParamMalformed(format!("digit_pubkey.[{}]", i))
                    })
            })
            .collect::<Result<Vec<[u8; 20]>, eyre::Report>>()
            .map_err(Into::into)
    }
}

impl TryFrom<SchnorrSig> for Signature {
    type Error = BridgeError;

    fn try_from(value: SchnorrSig) -> Result<Self, Self::Error> {
        Signature::from_slice(&value.schnorr_sig)
            .wrap_err("Failed to parse schnorr signature")
            .wrap_err_with(||ParserError::RPCParamMalformed("schnorr_sig".to_string()))
            .map_err(Into::into)
    }
}
impl From<winternitz::PublicKey> for WinternitzPubkey {
    fn from(value: winternitz::PublicKey) -> Self {
        {
            let digit_pubkey = value.into_iter().map(|inner| inner.to_vec()).collect();

            WinternitzPubkey { digit_pubkey }
        }
    }
}

impl From<Txid> for clementine::Txid {
    fn from(value: Txid) -> Self {
        {
            let txid = value.to_byte_array().to_vec();

            clementine::Txid { txid }
        }
    }
}
impl TryFrom<clementine::Txid> for Txid {
    type Error = FromSliceError;

    fn try_from(value: clementine::Txid) -> Result<Self, Self::Error> {
        {
            let txid = value.txid;

            Ok(Txid::from_raw_hash(sha256d::Hash::from_slice(&txid)?))
        }
    }
}

pub fn parse_deposit_params(
    deposit_params: clementine::DepositParams,
) -> Result<DepositData, Status> {
    let deposit_outpoint: bitcoin::OutPoint = deposit_params
        .deposit_outpoint
        .ok_or(Status::invalid_argument("No deposit outpoint received"))?
        .try_into()?;
    let evm_address: EVMAddress = deposit_params.evm_address.try_into().map_err(|e| {
        Status::invalid_argument(format!(
            "Failed to convert evm_address to EVMAddress: {}",
            e
        ))
    })?;
    let recovery_taproot_address = deposit_params
        .recovery_taproot_address
        .parse::<bitcoin::Address<_>>()
        .map_err(|e| Status::internal(e.to_string()))?;

    let nofn_xonly_pk: XOnlyPublicKey =
        XOnlyPublicKey::from_slice(&deposit_params.nofn_xonly_pk)
            .map_err(|e| BridgeError::Error(format!("Failed to parse xonly public key: {}", e)))?;

    Ok(DepositData {
        deposit_outpoint,
        evm_address,
        recovery_taproot_address,
        nofn_xonly_pk,
    })
}

pub fn parse_transaction_request(
    request: TransactionRequest,
) -> Result<TransactionRequestData, Status> {
    let deposit_data = parse_deposit_params(
        request
            .deposit_params
            .ok_or(Status::invalid_argument("No deposit params received"))?,
    )?;
    let transaction_type_proto = request
        .transaction_type
        .ok_or(Status::invalid_argument("No transaction type received"))?;
    let transaction_type: TransactionType = transaction_type_proto.try_into().map_err(|_| {
        Status::invalid_argument(format!(
            "Could not parse transaction type: {:?}",
            transaction_type_proto
        ))
    })?;
    let kickoff_id = request
        .kickoff_id
        .ok_or(Status::invalid_argument("No kickoff params received"))?;

    Ok(TransactionRequestData {
        deposit_data,
        transaction_type,
        kickoff_id,
    })
}

pub fn parse_assert_request(request: AssertRequest) -> Result<AssertRequestData, Status> {
    let deposit_data = parse_deposit_params(
        request
            .deposit_params
            .ok_or(Status::invalid_argument("No deposit params received"))?,
    )?;
    let kickoff_id = request
        .kickoff_id
        .ok_or(Status::invalid_argument("No kickoff params received"))?;

    Ok(AssertRequestData {
        deposit_data,
        kickoff_id,
    })
}

#[cfg(test)]
mod tests {
    use crate::rpc::clementine::{self, Outpoint, WinternitzPubkey};
    use bitcoin::{hashes::Hash, OutPoint, Txid};
    use bitvm::signatures::winternitz;

    #[test]
    fn from_bitcoin_outpoint_to_proto_outpoint() {
        let og_outpoint = OutPoint {
            txid: Txid::from_raw_hash(Hash::from_slice(&[0x1F; 32]).unwrap()),
            vout: 0x45,
        };

        let proto_outpoint: Outpoint = og_outpoint.into();
        let bitcoin_outpoint: OutPoint = proto_outpoint.try_into().unwrap();
        assert_eq!(og_outpoint, bitcoin_outpoint);

        let proto_outpoint = Outpoint {
            txid: vec![0x1F; 32],
            vout: 0x45,
        };
        let bitcoin_outpoint: OutPoint = proto_outpoint.try_into().unwrap();
        assert_eq!(og_outpoint, bitcoin_outpoint);
    }

    #[test]
    fn from_proto_outpoint_to_bitcoin_outpoint() {
        let og_outpoint = Outpoint {
            txid: vec![0x1F; 32],
            vout: 0x45,
        };

        let bitcoin_outpoint: OutPoint = og_outpoint.clone().try_into().unwrap();
        let proto_outpoint: Outpoint = bitcoin_outpoint.into();
        assert_eq!(og_outpoint, proto_outpoint);

        let bitcoin_outpoint = OutPoint {
            txid: Txid::from_raw_hash(Hash::from_slice(&[0x1F; 32]).unwrap()),
            vout: 0x45,
        };
        let proto_outpoint: Outpoint = bitcoin_outpoint.into();
        assert_eq!(og_outpoint, proto_outpoint);
    }

    #[test]
    fn from_proto_winternitz_public_key_to_bitvm() {
        let og_wpk = vec![[0x45u8; 20]];

        let rpc_wpk: WinternitzPubkey = og_wpk.clone().into();
        let rpc_converted_wpk: winternitz::PublicKey =
            rpc_wpk.try_into().expect("encoded wpk has to be valid");
        assert_eq!(og_wpk, rpc_converted_wpk);
    }

    #[test]
    fn from_txid_to_proto_txid() {
        let og_txid = Txid::from_raw_hash(Hash::from_slice(&[0x1F; 32]).unwrap());

        let rpc_txid: clementine::Txid = og_txid.into();
        let rpc_converted_txid: Txid = rpc_txid.try_into().unwrap();
        assert_eq!(og_txid, rpc_converted_txid);
    }
}
