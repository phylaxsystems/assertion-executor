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
    println!("caller: {CALLER}");
    // Deploys contracts, loads assertion store, and sets up state dependencies
    let mut executor = setup();

    let transactions: Vec<TxEnv> = generate_txs();

    let tx_count = transactions.len();

    let block_env = BlockEnv {
        basefee: uint!(1_U256),
        ..Default::default()
    };

    let avg_duration_no_assertion = benchmark_avg(BENCHRUNS, || {
        bench_no_assertion_execution(&mut executor, transactions.clone(), BlockEnv::default())
    });

    let avg_duration = benchmark_avg(BENCHRUNS, || {
        bench_execution(&mut executor, transactions.clone(), BlockEnv::default())
    });

    let total_gas_used = assert_txs_are_valid_and_compute_gas(
        &mut executor,
        transactions.clone(),
        block_env.clone(),
    );

    print!(
        "
    ~~~~~~~~~~~~~~~~~~~~~~~~~
    ~                       ~
    ~ Benchmarking Complete ~
    ~                       ~
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
        "> {} assertions total ran against {} transactions\n",
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
        "> Average time elapsed in validating {} transactions: {:?}\n",
        tx_count, avg_duration
    );

    println!(
        "> Average time per transaction: {:?}\n",
        if tx_count == 0 {
            Default::default()
        } else {
            avg_duration / tx_count as u32
        }
    );

    let diff = avg_duration - avg_duration_no_assertion;
    println!("> Total time of running assertions: {:?}\n", diff);
}
