use bitcoin::secp256k1::rand::rngs::OsRng;
use operator::config::{NUM_USERS, NUM_VERIFIERS};
use operator::constant::EVMAddress;
use operator::errors::BridgeError;
use operator::{extended_rpc::ExtendedRpc, operator::Operator, user::User};
use secp256k1::XOnlyPublicKey;

fn test_flow() -> Result<(), BridgeError> {
    let rpc = ExtendedRpc::new();

    let secp = bitcoin::secp256k1::Secp256k1::new();
    let mut all_xonly_pks = Vec::new();
    let mut all_sks = Vec::new();
    let rng = &mut OsRng;
    for _ in 0..NUM_VERIFIERS + 1 {
        let (sk, pk) = secp.generate_keypair(rng);
        all_xonly_pks.push(XOnlyPublicKey::from(pk));
        all_sks.push(sk);
    }

    let mut operator = Operator::new(&mut OsRng, &rpc, all_xonly_pks.clone(), all_sks)?;

    let mut users = Vec::new();
    for _ in 0..NUM_USERS {
        let (sk, _) = secp.generate_keypair(rng);
        users.push(User::new(&rpc, all_xonly_pks.clone(), sk));
    }

    // Initial setup for connector roots
    let (first_source_utxo, start_blockheight) = operator.initial_setup().unwrap();

    let mut connector_tree_source_sigs = Vec::new();

    for verifier in &mut operator.mock_verifier_access {
        let sigs = verifier.connector_roots_created(
            &operator.operator_mock_db.get_connector_tree_hashes(),
            start_blockheight,
            &first_source_utxo,
        );
        connector_tree_source_sigs.push(sigs);
    }

    println!("connector roots created, verifiers agree");
    // In the end, create BitVM

    // every user makes a deposit.
    for i in 0..NUM_USERS {
        let user = &users[i];
        // let user_evm_address = user.signer.evm_address;
        // println!("user_evm_address: {:?}", user_evm_address);
        // println!("move_utxo: {:?}", move_utxo);
        // let move_tx = rpc.get_raw_transaction(&move_utxo.txid, None).unwrap();
        // println!("move_tx: {:?}", move_tx);
        let evm_address: EVMAddress = [0; 20];
        let (deposit_utxo, deposit_return_address, user_evm_address, user_sig) =
            user.deposit_tx(evm_address).unwrap();
        rpc.mine_blocks(6)?;
        operator.new_deposit(
            deposit_utxo,
            &deposit_return_address,
            &user_evm_address,
            user_sig,
        )?;
        rpc.mine_blocks(1)?;
    }

    // make 3 withdrawals
    for i in 0..3 {
        operator.new_withdrawal(users[i].signer.address.clone())?;
        rpc.mine_blocks(1)?;
    }

    operator.inscribe_connector_tree_preimages()?;

    Ok(())
}

fn main() {
    test_flow().unwrap();
}
