use assertion_executor::{
    db::SharedDB,
    primitives::{
        BlockEnv,
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
    let mut fork_db = executor.db.fork();

    let start = Instant::now();
    transactions.into_iter().for_each(|tx| {
        let _ = executor.validate_transaction(block_env.clone(), tx, &mut fork_db);
    });
    start.elapsed()
}

/// Counts the number of valid assertions
pub fn count_valid_results(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> usize {
    let mut fork_db = executor.db.fork();

    transactions
        .into_iter()
        .filter_map(|tx| {
            executor
                .validate_transaction(block_env.clone(), tx, &mut fork_db)
                .unwrap()
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
    let total_duration = (0..iterations)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .sum::<Duration>();

    total_duration / iterations
}
