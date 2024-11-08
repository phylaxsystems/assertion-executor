mod bench;
mod setup;

use bench::*;
use setup::*;

use assertion_executor::primitives::{
    BlockEnv,
    TxEnv,
};

fn main() {
    // Deploys contracts, loads assertion store, and sets up state dependencies
    let mut executor = setup();

    let transactions: Vec<TxEnv> = generate_txs();

    let tx_count = transactions.len();

    let mut counter = 0;

    let avg_duration_no_assertion = benchmark_avg(100, || {
        bench_no_assertion_execution(&mut executor, transactions.clone(), BlockEnv::default())
    });

    let avg_duration = benchmark_avg(100, || {
        // println!("Benchmark run: {}", counter);
        counter += 1;
        bench_execution(&mut executor, transactions.clone(), BlockEnv::default())
    });

    print!(
        "
    =========================
    = Benchmarking complete =
    =========================\n\n"
    );

    // Calculate TPS
    let duration_seconds = avg_duration.as_secs_f64();
    let tps = if duration_seconds > 0.0 {
        tx_count as f64 / duration_seconds
    } else {
        0.0
    };

    // Calculate TPS no assertions
    let duration_seconds_no_assertion = avg_duration_no_assertion.as_secs_f64();
    let tps_no_assertion = if duration_seconds_no_assertion > 0.0 {
        tx_count as f64 / duration_seconds_no_assertion
    } else {
        0.0
    };

    // Collect metrics
    let assertions_ran = count_assertions(&mut executor, transactions.clone(), BlockEnv::default());
    // let valid_results =
    //     count_valid_results(&mut executor, transactions.clone(), BlockEnv::default());

    // Print results
    println!(
        "> {} assertions ran against {} transactions\n",
        assertions_ran, tx_count
    );

    // println!(
    //     "> {} invalidative transactions out of {} transactions\n",
    //     tx_count - valid_results,
    //     tx_count
    // );

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

    println!("> Transactions per second (TPS): {:.2}\n", tps);

    println!(
        "> Average time to execute transactions without running assertions: {:?}\n",
        avg_duration_no_assertion
    );

    println!(
        "> Transactions per second (TPS) without running assertions: {:.2}\n",
        tps_no_assertion
    );

    let diff = avg_duration - avg_duration_no_assertion;
    println!(
        "> Difference in time between running assertions and not running assertions: {:?}\n",
        diff
    );
}
