use assertion_executor::{
    db::SharedDB,
    primitives::{
        BlockEnv,
        ExecutionResult,
        TxEnv,
    },
    AssertionExecutor,
};

use std::time::{
    Duration,
    Instant,
};

/// Benchmarks the execution of transactions & assertions
pub fn bench_execution(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> Duration {
    println!("Executing {} transactions", transactions.len());
    let mut fork_db = executor.db.fork();

    let start = Instant::now();
    transactions.into_iter().for_each(|tx| {
        let _ = executor.validate_transaction(block_env.clone(), tx, &mut fork_db);
    });
    start.elapsed()
}

/// Assert transactions are valid
/// Return total gas used
pub fn assert_txs_are_valid_and_compute_gas(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> u64 {
    let mut fork_db = executor.db.fork();

    let mut total_gas_used = 0;

    transactions
        .into_iter()
        .map(|tx| {
            executor
                .execute_forked_tx(block_env.clone(), tx, &mut fork_db)
                .unwrap()
                .1
        })
        .for_each(|result_and_state| {
            let gas_used = match result_and_state.result {
                ExecutionResult::Revert { output, .. } => {
                    panic!("Transaction reverted: {output:#?}")
                }
                ExecutionResult::Halt { reason, .. } => {
                    panic!("TransactionHalted: {reason:#?}")
                }
                ExecutionResult::Success { gas_used, .. } => gas_used,
            };

            total_gas_used += gas_used;

            println!("Gas used: {}", gas_used);
        });

    total_gas_used
}

///

/// Benchmarks the execution of transactions
pub fn bench_no_assertion_execution(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> Duration {
    let mut fork_db = executor.db.fork();

    let start = Instant::now();
    transactions.into_iter().for_each(|tx| {
        let _ = executor.execute_forked_tx(block_env.clone(), tx, &mut fork_db);
    });

    start.elapsed()
}

/// Counts the number of valid assertions
pub fn count_valid_assertion_results(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> usize {
    let mut fork_db = executor.db.fork();

    transactions
        .into_iter()
        .filter_map(|tx| {
            let res = executor
                .validate_transaction(block_env.clone(), tx.clone(), &mut fork_db)
                .unwrap();
            if res.is_none() {
                println!("{:#?}", &tx);
            };
            res
        })
        .count()
}

/// Counts the number of assertions ran against a set of transactions
pub fn count_assertions(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> usize {
    let mut fork_db = executor.db.fork();

    transactions.into_iter().fold(0, |acc, tx| {
        let call_traces = executor
            .execute_forked_tx(block_env.clone(), tx, &mut fork_db)
            .unwrap()
            .0;

        let mut assertion_store_reader = executor.assertion_store_reader.clone();

        let assertions = assertion_store_reader
            .read_sync(block_env.number, call_traces)
            .expect("Failed to send request")
            .expect("Failed to receive response, channel empty and closed");

        acc + assertions
            .into_iter()
            .fold(0, |acc, contract| acc + contract.fn_selectors.len())
    })
}

/// Benchmarks the average time taken to execute a function
pub fn benchmark_avg<F, T>(iterations: u32, mut f: F) -> Duration
where
    F: FnMut() -> T,
{
    println!("Starting benchmark with {} iterations", iterations);
    let mut durations = Vec::with_capacity(iterations as usize);

    for i in 0..iterations {
        println!("Starting iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        f();
        let elapsed = start.elapsed();
        durations.push(elapsed);
    }

    println!("All iterations completed, calculating average");
    let total_duration: Duration = durations.iter().sum();
    let avg_duration = total_duration / iterations;
    println!("Average calculation completed: {:?}", avg_duration);

    // Print individual iteration times to spot any anomalies
    for (i, duration) in durations.iter().enumerate() {
        if *duration > avg_duration * 2 {
            println!(
                "Warning: Iteration {} took {:?} (more than 2x average)",
                i, duration
            );
        }
    }

    avg_duration
}
