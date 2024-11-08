use crate::fork_test::counter_call;
use assertion_executor::primitives::TxEnv;

/// Generates transactions to be executed in the benchmarking workload
pub fn generate_txs() -> Vec<TxEnv> {
    let mut transactions = Vec::new();

    for _ in 0..500 {
        transactions.push(counter_call());
    }

    transactions
}
