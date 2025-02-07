//! # Script Builder
//!
//! Script builder provides useful functions for building typical Bitcoin
//! scripts.
// Currently generate_witness functions are not yet used.
#![allow(dead_code)]

use crate::EVMAddress;
use bitcoin::opcodes::OP_TRUE;
use bitcoin::secp256k1::schnorr;
use bitcoin::{
    opcodes::{all::*, OP_FALSE},
    script::Builder,
    ScriptBuf, XOnlyPublicKey,
};
use bitcoin::{Amount, Witness};
use bitvm::signatures::winternitz::{self, SecretKey};
use bitvm::signatures::winternitz::{Parameters, PublicKey};
use std::any::Any;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, Copy, Clone)]
pub enum SpendPath {
    ScriptSpend(usize),
    KeySpend,
    Unknown,
}

pub trait SpendableScript: Send + Sync + 'static + std::any::Any {
    fn as_any(&self) -> &dyn Any;

    fn to_script_buf(&self) -> ScriptBuf;
}

impl Debug for dyn SpendableScript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SpendableScript")
    }
}

/// Struct for scripts that do not conform to any other type of SpendableScripts
#[derive(Debug, Clone)]
pub struct OtherSpendable(ScriptBuf);

impl From<ScriptBuf> for OtherSpendable {
    fn from(script: ScriptBuf) -> Self {
        Self(script)
    }
}

impl SpendableScript for OtherSpendable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        self.0.clone()
    }
}

impl OtherSpendable {
    fn as_script(&self) -> &ScriptBuf {
        &self.0
    }

    fn generate_script_inputs(&self, witness: Witness) -> Witness {
        witness
    }

    pub fn new(script: ScriptBuf) -> Self {
        Self(script)
    }
}

/// Struct for scripts that only includes a CHECKSIG
#[derive(Debug, Clone)]
pub struct CheckSig(pub(crate) XOnlyPublicKey);
impl SpendableScript for CheckSig {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        Builder::new()
            .push_x_only_key(&self.0)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }
}

impl CheckSig {
    pub fn generate_script_inputs(&self, signature: &schnorr::Signature) -> Witness {
        Witness::from_slice(&[signature.serialize()])
    }

    pub fn new(xonly_pk: XOnlyPublicKey) -> Self {
        Self(xonly_pk)
    }
}

/// Struct for scripts that commit to a message using Winternitz keys
#[derive(Clone)]
pub struct WinternitzCommit(PublicKey, Parameters, pub(crate) XOnlyPublicKey);
impl SpendableScript for WinternitzCommit {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        let winternitz_pubkey = self.0.clone();
        let params = self.1.clone();
        let xonly_pubkey = self.2;
        let verifier = winternitz::Winternitz::<
            winternitz::ListpickVerifier,
            winternitz::TabledConverter,
        >::new();
        verifier
            .checksig_verify(&params, &winternitz_pubkey)
            .push_x_only_key(&xonly_pubkey)
            .push_opcode(OP_CHECKSIG)
            .compile()
    }
}

impl WinternitzCommit {
    pub fn generate_script_inputs(
        &self,
        commit_data: &Vec<u8>,
        secret_key: &SecretKey,
        signature: &schnorr::Signature,
    ) -> Witness {
        let verifier = winternitz::Winternitz::<
            winternitz::ListpickVerifier,
            winternitz::TabledConverter,
        >::new();
        let mut witness = verifier.sign(&self.1, secret_key, &commit_data);
        witness.push(signature.serialize());
        witness
    }

    pub fn new(pubkey: PublicKey, params: Parameters, xonly_pubkey: XOnlyPublicKey) -> Self {
        Self(pubkey, params, xonly_pubkey)
    }
}

/// Struct for scripts that include a relative timelock (by block count) and optionally a CHECKSIG if a pubkey is provided.
/// Generates a relative timelock script with a given [`XOnlyPublicKey`] that CHECKSIG checks the signature against.
///
/// ATTENTION: If you want to spend a UTXO using timelock script, the
/// condition is that (`# in the script`) ≤ (`# in the sequence of the tx`)
/// ≤ (`# of blocks mined after UTXO appears on the chain`). However, this is not mandatory.
/// One can spend an output delayed for some number of blocks just by using the nSequence field
/// of the input inside the transaction. For more, see:
///
/// - [BIP-0068](https://github.com/bitcoin/bips/blob/master/bip-0068.mediawiki)
/// - [BIP-0112](https://github.com/bitcoin/bips/blob/master/bip-0112.mediawiki)
#[derive(Debug, Clone)]
pub struct TimelockScript(pub(crate) Option<XOnlyPublicKey>, u16);

impl SpendableScript for TimelockScript {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        let script_builder = Builder::new()
            .push_int(self.1 as i64)
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP);

        if let Some(xonly_pk) = self.0 {
            script_builder
                .push_x_only_key(&xonly_pk)
                .push_opcode(OP_CHECKSIG)
        } else {
            script_builder.push_opcode(OP_TRUE)
        }
        .into_script()
    }
}

impl TimelockScript {
    pub fn generate_script_inputs(&self, signature: &Option<schnorr::Signature>) -> Witness {
        match signature {
            Some(sig) => Witness::from_slice(&[sig.serialize()]),
            None => Witness::default(),
        }
    }

    pub fn new(xonly_pk: Option<XOnlyPublicKey>, block_count: u16) -> Self {
        Self(xonly_pk, block_count)
    }
}

/// Struct for scripts that reveal a preimage and verify it against a hash.
pub struct PreimageRevealScript(pub(crate) XOnlyPublicKey, [u8; 20]);

impl SpendableScript for PreimageRevealScript {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        Builder::new()
            .push_opcode(OP_HASH160)
            .push_slice(self.1)
            .push_opcode(OP_EQUALVERIFY)
            .push_x_only_key(&self.0)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }
}

impl PreimageRevealScript {
    pub fn generate_script_inputs(
        &self,
        preimage: impl AsRef<[u8]>,
        signature: &schnorr::Signature,
    ) -> Witness {
        let mut witness = Witness::from_slice(&[preimage]);
        witness.push(signature.serialize());
        witness
    }

    pub fn new(xonly_pk: XOnlyPublicKey, hash: [u8; 20]) -> Self {
        Self(xonly_pk, hash)
    }
}

/// Struct for deposit script that commits Citrea address to be deposited into onchain.
pub struct DepositScript(pub(crate) XOnlyPublicKey, EVMAddress, Amount);

impl SpendableScript for DepositScript {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_script_buf(&self) -> ScriptBuf {
        let citrea: [u8; 6] = "citrea".as_bytes().try_into().expect("length == 6");

        Builder::new()
            .push_x_only_key(&self.0)
            .push_opcode(OP_CHECKSIG)
            .push_opcode(OP_FALSE)
            .push_opcode(OP_IF)
            .push_slice(citrea)
            .push_slice(self.1 .0)
            .push_slice(self.2.to_sat().to_be_bytes())
            .push_opcode(OP_ENDIF)
            .into_script()
    }
}

impl DepositScript {
    pub fn generate_script_inputs(&self, signature: &schnorr::Signature) -> Witness {
        Witness::from_slice(&[signature.serialize()])
    }

    pub fn new(xonly_pk: XOnlyPublicKey, evm_address: EVMAddress, amount: Amount) -> Self {
        Self(xonly_pk, evm_address, amount)
    }
}

#[derive(Clone)]
pub enum ScriptKind<'a> {
    CheckSig(&'a CheckSig),
    WinternitzCommit(&'a WinternitzCommit),
    TimelockScript(&'a TimelockScript),
    PreimageRevealScript(&'a PreimageRevealScript),
    DepositScript(&'a DepositScript),
    Other(&'a OtherSpendable),
}

impl<'a> From<&'a Arc<dyn SpendableScript>> for ScriptKind<'a> {
    fn from(script: &'a Arc<dyn SpendableScript>) -> ScriptKind<'a> {
        let type_id = script.as_any().type_id();

        if type_id == std::any::TypeId::of::<CheckSig>() {
            Self::CheckSig(script.as_any().downcast_ref().expect("just checked"))
        } else if type_id == std::any::TypeId::of::<WinternitzCommit>() {
            Self::WinternitzCommit(script.as_any().downcast_ref().expect("just checked"))
        } else if type_id == std::any::TypeId::of::<TimelockScript>() {
            Self::TimelockScript(script.as_any().downcast_ref().expect("just checked"))
        } else if type_id == std::any::TypeId::of::<PreimageRevealScript>() {
            Self::PreimageRevealScript(script.as_any().downcast_ref().expect("just checked"))
        } else if type_id == std::any::TypeId::of::<DepositScript>() {
            Self::DepositScript(script.as_any().downcast_ref().expect("just checked"))
        } else {
            Self::Other(script.as_any().downcast_ref().expect("just checked"))
        }
    }
}

#[cfg(test)]
fn get_script_from_arr<T: SpendableScript>(
    arr: &Vec<Box<dyn SpendableScript>>,
) -> Option<(usize, &T)> {
    arr.iter()
        .enumerate()
        .find_map(|(i, x)| x.as_any().downcast_ref::<T>().map(|x| (i, x)))
}

#[test]
fn test_dynamic_casting() {
    use crate::utils;
    let scripts: Vec<Box<dyn SpendableScript>> = vec![
        Box::new(OtherSpendable(ScriptBuf::from_hex("51").expect(""))),
        Box::new(CheckSig(*utils::UNSPENDABLE_XONLY_PUBKEY)),
    ];

    let otherspendable = scripts
        .first()
        .expect("")
        .as_any()
        .downcast_ref::<OtherSpendable>()
        .expect("");

    let checksig = get_script_from_arr::<CheckSig>(&scripts).expect("");
    println!("{:?}", otherspendable);
    println!("{:?}", checksig);
}
