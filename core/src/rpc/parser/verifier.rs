use super::{convert_int_to_another, ParserError};
use crate::builder::transaction::DepositData;
use crate::citrea::CitreaClientT;
use crate::errors::BridgeError;
use crate::fetch_next_optional_message_from_stream;
use crate::rpc::clementine::{
    nonce_gen_response, verifier_deposit_sign_params, DepositSignSession, NonceGenFirstResponse,
    OperatorKeys, OperatorKeysWithDeposit, PartialSig, VerifierDepositSignParams, VerifierParams,
};
use crate::verifier::Verifier;
use crate::{
    fetch_next_message_from_stream,
    rpc::{
        clementine::{
            self, verifier_deposit_finalize_params, NonceGenResponse,
            VerifierDepositFinalizeParams, VerifierPublicKeys,
        },
        error::{self, invalid_argument},
    },
};
use bitcoin::secp256k1::schnorr;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::PublicKey;
use eyre::Context;
use secp256k1::musig::{MusigAggNonce, MusigPartialSignature, MusigPubNonce};
use tonic::Status;

impl<C> TryFrom<&Verifier<C>> for VerifierParams
where
    C: CitreaClientT,
{
    type Error = Status;

    fn try_from(verifier: &Verifier<C>) -> Result<Self, Self::Error> {
        let id = futures::executor::block_on(async {
            match *verifier.idx.read().await {
                Some(idx) => convert_int_to_another("id", idx, u32::try_from).map(Some),
                None => Ok(None),
            }
        })?;

        Ok(VerifierParams {
            id,
            public_key: verifier.signer.public_key.serialize().to_vec(),
            num_verifiers: convert_int_to_another(
                "num_verifiers",
                verifier.config.num_verifiers,
                u32::try_from,
            )?,
            num_operators: convert_int_to_another(
                "num_operators",
                verifier.config.num_operators,
                u32::try_from,
            )?,
            num_round_txs: convert_int_to_another(
                "num_round_txs",
                verifier.config.protocol_paramset().num_round_txs,
                u32::try_from,
            )?,
        })
    }
}

impl TryFrom<VerifierPublicKeys> for Vec<PublicKey> {
    type Error = BridgeError;

    fn try_from(value: VerifierPublicKeys) -> Result<Self, Self::Error> {
        let inner = value.verifier_public_keys;

        Ok(inner
            .iter()
            .map(|inner_vec| {
                PublicKey::from_slice(inner_vec).wrap_err_with(|| {
                    ParserError::RPCParamMalformed("verifier_public_keys".to_string())
                })
            })
            .collect::<Result<Vec<PublicKey>, eyre::Report>>()?)
    }
}
impl From<Vec<PublicKey>> for VerifierPublicKeys {
    fn from(value: Vec<PublicKey>) -> Self {
        let verifier_public_keys: Vec<Vec<u8>> = value
            .into_iter()
            .map(|inner| inner.serialize().to_vec())
            .collect();

        VerifierPublicKeys {
            verifier_public_keys,
        }
    }
}

impl From<DepositSignSession> for VerifierDepositSignParams {
    fn from(value: DepositSignSession) -> Self {
        VerifierDepositSignParams {
            params: Some(verifier_deposit_sign_params::Params::DepositSignFirstParam(
                value,
            )),
        }
    }
}

impl From<DepositSignSession> for VerifierDepositFinalizeParams {
    fn from(value: DepositSignSession) -> Self {
        VerifierDepositFinalizeParams {
            params: Some(
                verifier_deposit_finalize_params::Params::DepositSignFirstParam(value.clone()),
            ),
        }
    }
}

impl From<&Signature> for VerifierDepositFinalizeParams {
    fn from(value: &Signature) -> Self {
        VerifierDepositFinalizeParams {
            params: Some(verifier_deposit_finalize_params::Params::SchnorrSig(
                value.serialize().to_vec(),
            )),
        }
    }
}

impl From<NonceGenFirstResponse> for NonceGenResponse {
    fn from(value: NonceGenFirstResponse) -> Self {
        NonceGenResponse {
            response: Some(nonce_gen_response::Response::FirstResponse(value)),
        }
    }
}

impl From<&MusigPubNonce> for NonceGenResponse {
    fn from(value: &MusigPubNonce) -> Self {
        NonceGenResponse {
            response: Some(nonce_gen_response::Response::PubNonce(
                value.serialize().to_vec(),
            )),
        }
    }
}

impl From<MusigPartialSignature> for PartialSig {
    fn from(value: MusigPartialSignature) -> Self {
        PartialSig {
            partial_sig: value.serialize().to_vec(),
        }
    }
}

pub fn parse_deposit_sign_session(
    deposit_sign_session: clementine::DepositSignSession,
    verifier_idx: usize,
) -> Result<(DepositData, u32), Status> {
    let deposit_params = deposit_sign_session
        .deposit_params
        .ok_or(Status::invalid_argument("No deposit params received"))?;

    let deposit_data: DepositData = deposit_params.try_into()?;

    let session_id = deposit_sign_session.nonce_gen_first_responses[verifier_idx].id;

    Ok((deposit_data, session_id))
}

pub fn parse_partial_sigs(
    partial_sigs: Vec<Vec<u8>>,
) -> Result<Vec<MusigPartialSignature>, Status> {
    partial_sigs
        .iter()
        .enumerate()
        .map(|(idx, sig)| {
            MusigPartialSignature::from_slice(sig).map_err(|e| {
                error::invalid_argument(
                    "partial_sig",
                    format!("Verifier {idx} returned an invalid partial signature").as_str(),
                )(e)
            })
        })
        .collect::<Result<Vec<_>, _>>()
}

pub fn parse_op_keys_with_deposit(
    data: OperatorKeysWithDeposit,
) -> Result<(DepositData, OperatorKeys, u32), Status> {
    let deposit_params = data
        .deposit_params
        .ok_or(Status::invalid_argument("deposit_params is empty"))?;

    let deposit_data: DepositData = deposit_params.try_into()?;

    let op_keys = data
        .operator_keys
        .ok_or(Status::invalid_argument("OperatorDepositKeys is empty"))?;

    Ok((deposit_data, op_keys, data.operator_idx))
}

pub async fn parse_next_deposit_finalize_param_schnorr_sig(
    stream: &mut tonic::Streaming<VerifierDepositFinalizeParams>,
) -> Result<Option<schnorr::Signature>, Status> {
    let sig = match fetch_next_optional_message_from_stream!(stream, params) {
        Some(sig) => sig,
        None => return Ok(None),
    };

    let final_sig = match sig {
        verifier_deposit_finalize_params::Params::SchnorrSig(final_sig) => {
            schnorr::Signature::from_slice(&final_sig)
                .map_err(invalid_argument("FinalSig", "Invalid signature length"))?
        }
        _ => return Err(Status::internal("Expected FinalSig 1")),
    };

    Ok(Some(final_sig))
}

pub async fn parse_deposit_finalize_param_agg_nonce(
    stream: &mut tonic::Streaming<VerifierDepositFinalizeParams>,
) -> Result<MusigAggNonce, Status> {
    let sig = fetch_next_message_from_stream!(stream, params)?;

    match sig {
        verifier_deposit_finalize_params::Params::MoveTxAggNonce(aggnonce) => {
            Ok(MusigAggNonce::from_slice(&aggnonce)
                .map_err(invalid_argument("MusigAggNonce", "failed to parse"))?)
        }
        _ => Err(Status::internal("Expected FinalSig 2")),
    }
}

pub async fn parse_nonce_gen_first_response(
    stream: &mut tonic::Streaming<NonceGenResponse>,
) -> Result<clementine::NonceGenFirstResponse, Status> {
    let nonce_gen_response = fetch_next_message_from_stream!(stream, response)?;

    if let clementine::nonce_gen_response::Response::FirstResponse(nonce_gen_first_response) =
        nonce_gen_response
    {
        Ok(nonce_gen_first_response)
    } else {
        Err(Status::invalid_argument("Expected first_response"))
    }
}
