# `assertion-executor` benchmark

The following is a benchmark of the `assertion-executor`. It is a preliminary view of the performance and feature set expected for release.
It demonstrates:

- Regular transaction execution using `revm`
- Runtime assertion validation using `phEVM`
- State forking during assertion checks
- Automatic transaction filtering based on failed assertions

## Benchmark

The benchmark simulates a real bundle validation scenario by using a similar gas load as to what one might expect in a production enviroment on a network like `Base`.
A bundle of transactions is ran, some of which trigger assertions, and the resulting output is displayed.

The system uses two primary execution environments:

- **revm**: Handles standard transaction execution following production network rules
- **phEVM**: A specialized environment for assertion execution with features like:
  - Atomic execution
  - Massive parallelization
  - State forking capabilities
  - Custom cheatcodes for advanced state manipulation

### Benchmark details

The benchmark includes the following transactions:

- Regular transactions that do not trigger assertions (like EOA Eth sends);
- Transactions that try to change disallowed state similar to the radiant hack;
- Transactions that challange lending protocol solvency.

The second two items always trigger assertion due to interacting with Assertion Adopters (Protocols with assertions associated with them). The lending protocol triggers 2 assertions, one that checks withdraw invariants, and one that simulates complex lending protocol invariants by wasting gast. The disallowed state transaction triggers a single basic assertion.

All assertions are ran sequentially, **this benchmark demonstrates a worst case scenario for the assertion executor with all assertions running sequentially**.

#### Radiant simulation

We simulate a contract that is in essence similar to what happened with the radiant exploit. We have a state variable that an attacker with can change, and the contract holding the variable has no timelocks or any other forms of protection built-in to disallow immediate changes.

We send a transaction that changes the state to different value. We successfully execute the transaction with `revm`. Afterwards, we start running assertions using the `phEvm`. The assertions check if the value was changed. This is achieved by **forking** to the blockchain state before transaction execution, reading the relevant storage values, and switching back to see if any changes have been made.

The assertion fails if the value was changed, and removes the transaction changing the state from the bundle.

#### Lending solvency

We mock a lending protocol that accepts Eth deposits, withdrawals, and borrows. The protocol has multiple vulnerabilities, like reentrancy, and a general lack of invariant and logic checks.

We initiate a transaction that attempts to withdraw too much money from the protocol. Once the transaction suceesfully executes using `revm`, we run the assertions associated with the protocol using `phEvm`. The assertions check the account balance of the address initiating the transaction, and check the protocol solvency by making sure the balance of the account inside of the protocol is not more than the amount of money withdrawn.

The assertion fails if the balance of the account inside of the protocol is greater than the amount of money withdrawn, and removes the transaction from the bundle.

### Benchmark results

```bash
    ~~~~~~~~~~~~~~~~~~~~~~~~~
    ~                       ~
    ~ Benchmarking Complete ~
    ~                       ~
    ~~~~~~~~~~~~~~~~~~~~~~~~~


    =========================
    =       Work Load       =
    =========================

> 600 assertions total ran against 1600 transactions

> 1600 transactions consuming a total of 49,411,200 gas units

> 600 invalidative transactions out of 1600 transactions


    =========================
    =        Results        =
    =========================

> Average time elapsed in validating 1600 transactions: 62.888375ms

> Average time per transaction: 39.305µs

> Total time of running assertions: 41.016625ms
```

For consistency, the benchmark is ran 100 times against the same bundle of transactions.

The following output shows:

- The work load:
  - How many transactions and assertions were ran;
  - How much gas was consumed by the transactions;
  - How many transactions were invalidated (transactions with assertions that fail after exection).
- The results of the execution:
  - The average time for validating a bundle;
  - The average time it took to execute a transaction and associated assertion (`revm` + `phEVM`);
  - How much extra time it takes to run assertions compared to not running them (`phEVM` execution time).

## How to run

Nightly rust is required to run the benchmark. To run it execute the following:

```bash
cargo run --bin demo --profile maxperf
```

To guarantee a working dev enviroment, you can use the latest known good nightly with nix:

```bash
nix develop
cargo run --bin demo --profile maxperf
```
