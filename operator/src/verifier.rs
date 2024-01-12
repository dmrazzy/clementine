use std::collections::HashMap;

use bitcoin::{
    hashes::Hash, opcodes::OP_TRUE, script::Builder, secp256k1, secp256k1::Secp256k1, OutPoint, TapSighash,
};
use bitcoincore_rpc::Client;
use secp256k1::{rand::rngs::OsRng, XOnlyPublicKey};

use crate::utils::{create_btc_tx, create_tx_ins, create_tx_outs, generate_n_of_n_script, create_taproot_address};
use crate::{
    actor::Actor,
    operator::{check_deposit, DepositPresigns},
    user::User,
    utils::generate_n_of_n_script_without_hash,
};

use circuit_helpers::config::{EVMAddress, BRIDGE_AMOUNT_SATS, MIN_RELAY_FEE, NUM_ROUNDS};

pub struct Verifier<'a> {
    pub rpc: &'a Client,
    pub secp: Secp256k1<secp256k1::All>,
    pub signer: Actor,
    pub operator: XOnlyPublicKey,
    pub verifiers: Vec<XOnlyPublicKey>,
}

impl<'a> Verifier<'a> {
    pub fn new(rng: &mut OsRng, rpc: &'a Client, operator_pk: XOnlyPublicKey) -> Self {
        let signer = Actor::new(rng);
        let secp: Secp256k1<secp256k1::All> = Secp256k1::new();
        let verifiers = Vec::new();
        Verifier {
            rpc,
            secp,
            signer,
            operator: operator_pk,
            verifiers,
        }
    }

    pub fn set_verifiers(&mut self, verifiers: Vec<XOnlyPublicKey>) {
        self.verifiers = verifiers;
    }

    // this is a public endpoint that only depositor can call
    pub fn new_deposit(
        &self,
        utxo: OutPoint,
        hash: [u8; 32],
        return_address: XOnlyPublicKey,
        evm_address: EVMAddress,
    ) -> DepositPresigns {
        let mut all_verifiers = self.verifiers.to_vec();
        all_verifiers.push(self.operator);
        let timestamp = check_deposit(
            &self.secp,
            self.rpc,
            utxo,
            hash,
            return_address,
            &all_verifiers,
        );
        let script_n_of_n = generate_n_of_n_script(&all_verifiers, hash);

        let script_n_of_n_without_hash = generate_n_of_n_script_without_hash(&all_verifiers);
        let (address, _) = create_taproot_address(&self.signer.secp, vec![script_n_of_n_without_hash.clone()]);

        let script_anyone_can_spend = Builder::new().push_opcode(OP_TRUE).into_script();
        let anyone_can_spend_script_pub_key = script_anyone_can_spend.to_p2wsh();
        let dust_value = script_anyone_can_spend.dust_value();

        let kickoff_tx_ins = create_tx_ins(vec![utxo]);

        let kickoff_tx_outs = create_tx_outs(vec![
            (
                bitcoin::Amount::from_sat(BRIDGE_AMOUNT_SATS)
                    - dust_value
                    - bitcoin::Amount::from_sat(MIN_RELAY_FEE),
                address.script_pubkey(),
            ),
            (dust_value, anyone_can_spend_script_pub_key.clone()),
        ]);

        let mut kickoff_tx = create_btc_tx(kickoff_tx_ins, kickoff_tx_outs);

        let (deposit_address, _) =
            User::generate_deposit_address(&self.signer.secp, &all_verifiers, hash, return_address);

        let prevouts = create_tx_outs(vec![(bitcoin::Amount::from_sat(BRIDGE_AMOUNT_SATS), deposit_address.script_pubkey())]);

        let kickoff_sign = self.signer.sign_taproot_script_spend_tx(&mut kickoff_tx, prevouts, &script_n_of_n, 0);
        let kickoff_txid = kickoff_tx.txid();

        let mut prev_outpoint = OutPoint {
            txid: kickoff_txid,
            vout: 0,
        };
        let mut prev_amount = bitcoin::Amount::from_sat(BRIDGE_AMOUNT_SATS)
            - dust_value
            - bitcoin::Amount::from_sat(MIN_RELAY_FEE);

        let mut move_bridge_sign_utxo_pairs = HashMap::new();
        let mut operator_take_signs = Vec::new();

        for _ in 0..NUM_ROUNDS {
            let move_tx_ins = create_tx_ins(vec![prev_outpoint]);

            let move_tx_outs = create_tx_outs(vec![(
                prev_amount - dust_value - bitcoin::Amount::from_sat(MIN_RELAY_FEE),
                address.script_pubkey()
            ), (
                dust_value,
                anyone_can_spend_script_pub_key.clone()
            )]);

            let mut move_tx = create_btc_tx(move_tx_ins, move_tx_outs);

            let prevouts = create_tx_outs(vec![(prev_amount, address.script_pubkey())]);

            let move_fund_sign = self.signer.sign_taproot_script_spend_tx(&mut move_tx, prevouts, &script_n_of_n_without_hash, 0);

            move_bridge_sign_utxo_pairs.insert(prev_outpoint, move_fund_sign);
            operator_take_signs.push(self.signer.sign(TapSighash::all_zeros()));

            prev_outpoint = OutPoint {
                txid: move_tx.txid(),
                vout: 0,
            };
            prev_amount = prev_amount - dust_value - bitcoin::Amount::from_sat(MIN_RELAY_FEE);
        }

        let rollup_sign = self.signer.sign_deposit(
            kickoff_txid,
            evm_address,
            hash,
            timestamp.to_consensus_u32().to_be_bytes(),
        );
        DepositPresigns {
            rollup_sign,
            kickoff_sign,
            move_bridge_sign_utxo_pairs,
            operator_take_signs,
        }
    }

    // This is a function to reduce gas costs when moving bridge funds
    pub fn do_me_a_favor() {}
}
