mod bench;
use bench::*;

mod setup;
use setup::*;

use assertion_executor::primitives::{
    uint,
    BlockEnv,
    TxEnv,
};

#[tokio::main]
async fn main() {
    //Deploys contracts, loads assertion store, and sets up state dependencies
    let mut executor = setup().await;

    let transactions: Vec<TxEnv> = generate_txs();

    let tx_count = transactions.len();

    let block_env = BlockEnv {
        number: uint!(1_U256),
        ..Default::default()
    };

    let avg_duration = benchmark_avg(100, || {
        bench_execution(&mut executor, transactions.clone(), block_env.clone())
    });

    let assertions_ran = count_assertions(&mut executor, transactions.clone(), block_env.clone());

    let valid_results = count_valid_results(&mut executor, transactions, block_env);

    println!(
        "{} assertions ran against {} transactions",
        assertions_ran, tx_count
    );

    println!(
        "{} invalidative transactions out of {} transactions",
        tx_count - valid_results,
        tx_count
    );

    println!(
        "Average time elapsed in validating {} transactions: {:?}",
        tx_count, avg_duration
    );

    println!("Average time per transaction: {:?}", {
        if tx_count == 0 {
            Default::default()
        } else {
            avg_duration / tx_count as u32
        }
    });
}
