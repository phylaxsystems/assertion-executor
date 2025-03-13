use assertion_executor::{
    db::SharedDB,
    primitives::{
        BlockEnv,
        EvmExecutionResult,
        TxEnv,
    },
    AssertionExecutor,
};

use std::time::{
    Duration,
    Instant,
};

/// Benchmarks the execution of transactions & assertions
pub async fn bench_execution(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> Duration {
    let mut fork_db = executor.db.fork();

    let start = Instant::now();
    for tx in transactions {
        let _ = executor.validate_transaction(block_env.clone(), tx, &mut fork_db);
    }
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
                EvmExecutionResult::Revert { output, .. } => {
                    panic!("Transaction reverted: {output:#?}")
                }
                EvmExecutionResult::Halt { reason, .. } => {
                    panic!("TransactionHalted: {reason:#?}")
                }
                EvmExecutionResult::Success { gas_used, .. } => gas_used,
            };

            total_gas_used += gas_used;
        });

    total_gas_used
}

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
pub async fn count_valid_assertion_results(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> usize {
    let mut fork_db = executor.db.fork();

    let mut count = 0;
    for tx in transactions {
        if let Ok(res) = executor.validate_transaction(block_env.clone(), tx, &mut fork_db) {
            if res.is_valid() {
                count += 1;
            }
        }
    }
    count
}

/// Counts the number of assertions ran against a set of transactions
pub async fn count_assertions(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    transactions: Vec<TxEnv>,
    block_env: BlockEnv,
) -> usize {
    let mut fork_db = executor.db.fork();

    let mut count = 0;
    for tx in transactions {
        let call_traces = executor
            .execute_forked_tx(block_env.clone(), tx, &mut fork_db)
            .unwrap()
            .0;

        let assertions = executor
            .store
            .read(&call_traces, block_env.number)
            .expect("Failed to read assertions");

        count += assertions.len();
    }
    count
}

pub struct BenchMarkResult {
    pub avg_duration: Duration,
    pub min_duration: Duration,
    pub max_duration: Duration,
}

/// Benchmarks the average time taken to execute a function
pub async fn benchmark<F, T>(iterations: u32, mut f: F) -> BenchMarkResult
where
    F: AsyncFnMut() -> T,
{
    let mut durations = Vec::with_capacity(iterations as usize);

    for _ in 0..iterations {
        let start = Instant::now();
        f().await;
        let elapsed = start.elapsed();
        durations.push(elapsed);
    }

    let min_duration = durations.iter().min().unwrap();
    let max_duration = durations.iter().max().unwrap();
    let total_duration: Duration = durations.iter().sum();
    let avg_duration = total_duration / iterations;

    BenchMarkResult {
        avg_duration,
        min_duration: *min_duration,
        max_duration: *max_duration,
    }
}
