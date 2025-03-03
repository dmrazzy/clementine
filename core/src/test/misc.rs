use super::common::run_single_deposit;
use crate::extended_rpc::ExtendedRpc;
use bitcoincore_rpc::RpcApi;

use crate::test::common::*;

#[tokio::test]
async fn test_deposit() {
    let config = create_test_config_with_thread_name(None).await;

    // Start the timer
    let start_time = std::time::Instant::now();

    // Run the deposit
    run_single_deposit(config).await.unwrap();

    // Calculate and print the elapsed time
    let elapsed = start_time.elapsed();
    println!("run_single_deposit completed in: {:?}", elapsed);
}

//     #[ignore = "We are switching to gRPC"]
//     #[tokio::test]
//     async fn multiple_deposits_for_operator() {
//         run_multiple_deposits("test_config.toml").await;
//     }

#[tokio::test]
async fn create_regtest_rpc_macro() {
    let mut config = create_test_config_with_thread_name(None).await;

    let regtest = create_regtest_rpc(&mut config).await;

    let macro_rpc = regtest.rpc();
    let rpc = ExtendedRpc::connect(
        config.bitcoin_rpc_url.clone(),
        config.bitcoin_rpc_user.clone(),
        config.bitcoin_rpc_password.clone(),
    )
    .await
    .unwrap();

    macro_rpc.mine_blocks(1).await.unwrap();
    let height = macro_rpc.client.get_block_count().await.unwrap();
    let new_rpc_height = rpc.client.get_block_count().await.unwrap();
    assert_eq!(height, new_rpc_height);

    rpc.mine_blocks(1).await.unwrap();
    let new_rpc_height = rpc.client.get_block_count().await.unwrap();
    let height = macro_rpc.client.get_block_count().await.unwrap();
    assert_eq!(height, new_rpc_height);
}
