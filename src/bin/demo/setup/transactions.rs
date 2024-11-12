use crate::{
    demo_lending::{
        lending_call_invalidative,
        lending_call_valid,
    },
    fork_test::counter_call,
    CALLER,
};

use assertion_executor::primitives::{
    uint,
    TxEnv,
};

/// Generates transactions to be executed in the benchmarking workload
pub fn generate_txs() -> Vec<TxEnv> {
    let mut transactions = Vec::new();

    for _ in 0..300 {
        transactions.push(TxEnv {
            caller: CALLER,
            gas_price: uint!(1_U256),
            ..counter_call()
        });
    }

    for _ in 0..300 {
        transactions.push(TxEnv {
            caller: CALLER,
            gas_price: uint!(1_U256),
            ..lending_call_invalidative()
        });
    }

    //for _ in 0..300 {
    //    transactions.push(TxEnv {
    //        caller: CALLER,
    //        gas_price: uint!(1_U256),
    //        ..lending_call_valid()
    //    });
    //}

    let send_eth = TxEnv {
        value: uint!(100_U256),
        caller: CALLER,
        gas_price: uint!(1_U256),
        ..TxEnv::default()
    };

    for _ in 0..1000 {
        transactions.push(send_eth.clone())
    }

    transactions
}
