mod bench;
mod setup;

use bench::*;
use setup::*;

use assertion_executor::primitives::{
    uint,
    Address,
    BlockEnv,
    TxEnv,
};

// Do 1 run in debug mode, 100 runs when release
#[cfg(debug_assertions)]
const BENCHRUNS: u32 = 1;
#[cfg(not(debug_assertions))]
const BENCHRUNS: u32 = 100;

pub const CALLER: Address = Address::new([69u8; 20]);

fn main() {
    // Deploys contracts, loads assertion store, and sets up state dependencies
    let mut executor = setup();

    let transactions: Vec<TxEnv> = generate_txs();

    let tx_count = transactions.len();

    let block_env = BlockEnv {
        basefee: uint!(1_U256),
        ..Default::default()
    };

    let total_gas_used = assert_txs_are_valid_and_compute_gas(
        &mut executor,
        transactions.clone(),
        block_env.clone(),
    );

    let result_no_assertions = benchmark(BENCHRUNS, || {
        bench_no_assertion_execution(&mut executor, transactions.clone(), block_env.clone())
    });

    let result = benchmark(BENCHRUNS, || {
        bench_execution(&mut executor, transactions.clone(), block_env.clone())
    });

    print!(
        "
    ~~~~~~~~~~~~~~~~~~~~~~~~~
    ~~~~~~~~~~~~~~~~~~~~~~~~~
    ~ Benchmarking Complete ~
    ~~~~~~~~~~~~~~~~~~~~~~~~~
    ~~~~~~~~~~~~~~~~~~~~~~~~~\n\n"
    );

    // Collect metrics
    let assertions_ran = count_assertions(&mut executor, transactions.clone(), BlockEnv::default());

    let valid_results =
        count_valid_assertion_results(&mut executor, transactions.clone(), BlockEnv::default());

    // Print results
    print!(
        "
    =========================
    =       Work Load       =
    =========================\n\n"
    );

    println!(
        "> {} assertions ran against {} transactions\n",
        assertions_ran, tx_count
    );

    println!(
        "> {} transactions consuming a total of {} gas units\n",
        tx_count,
        total_gas_used
            .to_string()
            .as_bytes()
            .rchunks(3)
            .rev()
            .map(std::str::from_utf8)
            .collect::<Result<Vec<&str>, _>>()
            .unwrap()
            .join(",")
    );

    println!(
        "> {} invalidative transactions out of {} transactions\n",
        tx_count - valid_results,
        tx_count
    );

    print!(
        "
    =========================
    =        Results        =
    =========================\n\n"
    );

    println!(
        "> Time elapsed in executing {} transactions and {} assertions: Min: {:#?}, Max: {:#?}, Avg: {:#?}\n",
        tx_count, assertions_ran, result.min_duration, result.max_duration, result.avg_duration
    );

    println!(
        "> Time elapsed in executing {} transactions without assertions: Min: {:#?}, Max: {:#?}, Avg: {:#?}\n",
        tx_count, result_no_assertions.min_duration, result_no_assertions.max_duration, result_no_assertions.avg_duration
    );

    let diff = result.avg_duration - result_no_assertions.avg_duration;
    println!(
        "> Total time of running {} assertions: {:?}\n",
        assertions_ran, diff
    );
}
