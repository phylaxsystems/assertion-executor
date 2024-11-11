use crate::{
    demo_lending::lending_call,
    fork_test::counter_call,
};

use assertion_executor::primitives::TxEnv;

/// Generates transactions to be executed in the benchmarking workload
pub fn generate_txs() -> Vec<TxEnv> {
    let mut transactions = Vec::new();

    for _ in 0..300 {
        transactions.push(counter_call());
    }

    for _ in 0..300 {
        transactions.push(lending_call());
    }

    for _ in 0..2000 {
        transactions.push(TxEnv::default())
    }

    transactions
}
