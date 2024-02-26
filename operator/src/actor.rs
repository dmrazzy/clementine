use std::borrow::BorrowMut;

use crate::constant::EVMAddress;
use bitcoin::hashes::sha256;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::rand::RngCore;
use bitcoin::sighash::SighashCache;
use bitcoin::taproot::LeafVersion;
use bitcoin::{
    hashes::Hash,
    secp256k1::{
        ecdsa, schnorr, All, Keypair, Message, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey,
    },
    Address, TapSighash, TapTweakHash,
};

use bitcoin::{Amount, OutPoint, TapLeafHash, TapNodeHash, TxOut, Txid};
use bitcoincore_rpc::{Client, RpcApi};
use tiny_keccak::{Hasher, Keccak};

use crate::transaction_builder::TransactionBuilder;

#[derive(Clone, Debug, Copy)]
pub struct EVMSignature {
    pub v: u8,
    pub r: [u8; 32],
    pub s: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct Actor {
    pub secp: Secp256k1<All>,
    keypair: Keypair,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
    pub xonly_public_key: XOnlyPublicKey,
    pub address: Address,
    pub evm_address: EVMAddress,
}

impl Default for Actor {
    fn default() -> Self {
        Self::new(&mut OsRng)
    }
}

impl Actor {
    pub fn new<R: RngCore>(rng: &mut R) -> Self {
        let secp: Secp256k1<All> = Secp256k1::new();
        let (sk, pk) = secp.generate_keypair(rng);
        let keypair = Keypair::from_secret_key(&secp, &sk);
        let (xonly, _parity) = XOnlyPublicKey::from_keypair(&keypair);
        let address = Address::p2tr(&secp, xonly, None, bitcoin::Network::Regtest);

        let pk_serialized = pk.serialize_uncompressed();
        let pk_serialized: [u8; 64] = pk_serialized[1..].try_into().unwrap();
        let mut evm_address = [0u8; 32];
        let mut keccak_hasher = Keccak::v256();
        keccak_hasher.update(&pk_serialized);
        keccak_hasher.finalize(&mut evm_address);
        let evm_address: EVMAddress = evm_address[12..].try_into().unwrap();

        Actor {
            secp,
            keypair,
            secret_key: keypair.secret_key(),
            public_key: pk,
            xonly_public_key: xonly,
            address,
            evm_address,
        }
    }

    pub fn sign_with_tweak(
        &self,
        sighash: TapSighash,
        merkle_root: Option<TapNodeHash>,
    ) -> schnorr::Signature {
        self.secp.sign_schnorr(
            &Message::from_digest_slice(sighash.as_byte_array()).expect("should be hash"),
            &self
                .keypair
                .add_xonly_tweak(
                    &self.secp,
                    &TapTweakHash::from_key_and_tweak(self.xonly_public_key, merkle_root)
                        .to_scalar(),
                )
                .unwrap(),
        )
    }

    pub fn sign(&self, sighash: TapSighash) -> schnorr::Signature {
        self.secp.sign_schnorr(
            &Message::from_digest_slice(sighash.as_byte_array()).expect("should be hash"),
            &self.keypair,
        )
    }

    pub fn sign_ecdsa(&self, data: [u8; 32]) -> ecdsa::Signature {
        self.secp.sign_ecdsa(
            &Message::from_digest_slice(&data).expect("should be hash"),
            &self.secret_key,
        )
    }

    pub fn sign_taproot_script_spend_tx(
        &self,
        tx: &mut bitcoin::Transaction,
        prevouts: &Vec<TxOut>,
        spend_script: &bitcoin::Script,
        input_index: usize,
    ) -> schnorr::Signature {
        let mut sighash_cache = SighashCache::new(tx);
        let sig_hash = sighash_cache
            .taproot_script_spend_signature_hash(
                input_index,
                &bitcoin::sighash::Prevouts::All(prevouts),
                TapLeafHash::from_script(&spend_script, LeafVersion::TapScript),
                bitcoin::sighash::TapSighashType::Default,
            )
            .unwrap();
        self.sign(sig_hash)
    }

    pub fn sign_taproot_pubkey_spend_tx(
        &self,
        tx: &mut bitcoin::Transaction,
        prevouts: &Vec<TxOut>,
        input_index: usize,
    ) -> schnorr::Signature {
        let mut sighash_cache = SighashCache::new(tx);
        let sig_hash = sighash_cache
            .taproot_key_spend_signature_hash(
                input_index,
                &bitcoin::sighash::Prevouts::All(prevouts),
                bitcoin::sighash::TapSighashType::Default,
            )
            .unwrap();
        self.sign_with_tweak(sig_hash, None)
    }

    // pub fn verify_script_spend_signature(
    //     _tx: &bitcoin::Transaction,
    //     _presign: &schnorr::Signature,
    //     _xonly_public_key: &XOnlyPublicKey,
    //     spend_script: &bitcoin::Script,
    //     input_index: usize,
    //     prevouts: &Vec<TxOut>,
    // ) -> Option<bool> {
    //     let sighash_cache = SighashCache::new(_tx);
    //     let sig_hash = sighash_cache
    //         .taproot_script_spend_signature_hash(
    //             input_index,
    //             &bitcoin::sighash::Prevouts::All(&prevouts),
    //             TapLeafHash::from_script(&spend_script, LeafVersion::TapScript),
    //             bitcoin::sighash::TapSighashType::Default,
    //         )
    //         .unwrap();
        
    //     Some(true)
    // }

    pub fn create_self_utxo(&self, rpc: &Client, amount: Amount) -> (OutPoint, Amount) {
        let txid = rpc
            .send_to_address(&self.address, amount, None, None, None, None, None, None)
            .unwrap();
        let tx = rpc.get_raw_transaction(&txid, None).unwrap();
        let vout = tx.output.iter().position(|x| x.value == amount).unwrap();
        println!("user created start utxo: {:?}", txid);
        (
            OutPoint {
                txid,
                vout: vout as u32,
            },
            amount,
        )
    }

    pub fn spend_self_utxo(
        &self,
        rpc: &Client,
        utxo: OutPoint,
        amount: Amount,
        address: Address,
    ) -> (OutPoint, Amount) {
        let prev_tx = rpc.get_raw_transaction(&utxo.txid, None).unwrap();
        let prev_amount = prev_tx.output[utxo.vout as usize].value;

        let tx_ins = TransactionBuilder::create_tx_ins(vec![utxo]);
        let tx_outs = TransactionBuilder::create_tx_outs(vec![(amount, address.script_pubkey())]);
        let mut spend_tx = TransactionBuilder::create_btc_tx(tx_ins, tx_outs);

        let prevouts =
            TransactionBuilder::create_tx_outs(vec![(prev_amount, self.address.script_pubkey())]);

        let sig = self.sign_taproot_pubkey_spend_tx(&mut spend_tx, &prevouts, 0);
        let mut deposit_tx_sighash_cache = SighashCache::new(spend_tx.borrow_mut());
        let witness = deposit_tx_sighash_cache.witness_mut(0).unwrap();
        witness.push(sig.as_ref());

        let spend_txid = rpc
            .send_raw_transaction(&spend_tx)
            .unwrap_or_else(|e| panic!("Failed to send raw transaction: {}", e));
        println!("user spent start utxo: {:?}", utxo);
        println!("user spent txid: {:?}", spend_txid);
        (TransactionBuilder::create_utxo(spend_txid, 0), amount)
    }

    pub fn sign_deposit(
        &self,
        txid: Txid,
        evm_address: EVMAddress,
        hash: [u8; 32],
    ) -> EVMSignature {
        let mut message = [0; 84];
        message[..32].copy_from_slice(&txid.to_byte_array());
        message[32..52].copy_from_slice(&evm_address);
        message[52..84].copy_from_slice(&hash);

        let message = sha256::Hash::hash(&message);
        let signature = self.secp.sign_ecdsa_recoverable(
            &Message::from_digest_slice(&message.to_byte_array()).expect("should be hash"),
            &self.secret_key,
        );
        let (rec_id, signature): (ecdsa::RecoveryId, [u8; 64]) = signature.serialize_compact();
        let v = rec_id.to_i32() as u8 + 27;
        let r: [u8; 32] = signature[..32].try_into().unwrap();
        let s: [u8; 32] = signature[32..].try_into().unwrap();

        EVMSignature { v, r, s }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ecdsa() {
        let prover = Actor::new(&mut OsRng);
        let txid = Txid::all_zeros();
        let timestamp = [2; 4];
        let hash = [3; 32];
        let evm_address = prover.evm_address;

        let sig = prover.sign_deposit(txid, evm_address, hash);
        let v = sig.v;
        let r = sig.r;
        let s = sig.s;

        println!("bytes32 txid = bytes32(0x{});", hex::encode(txid));
        println!(
            "address deposit_address = address(bytes20(hex\"{}\"));",
            hex::encode(evm_address)
        );
        println!("bytes32 _hash = bytes32(0x{});", hex::encode(hash));
        println!("bytes4 timestamp = bytes4(0x{});", hex::encode(timestamp));
        println!("bytes32 r = bytes32(0x{});", hex::encode(r));
        println!("bytes32 s = bytes32(0x{});", hex::encode(s));
        println!("uint8 v = {};", v);
        println!(
            "address expected = address(bytes20(hex\"{}\"));",
            hex::encode(prover.evm_address)
        );
    }
}
