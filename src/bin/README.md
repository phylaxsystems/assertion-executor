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

The second two items always trigger assertion due to interacting with Assertion Adopters (Protocols with assertions associated with them).

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
    =========================
    = Benchmarking complete =
    =========================

> 600 assertions ran against 2600 transactions

> Average time elapsed in validating 2600 transactions: 51.155608ms

> Average time per transaction: 19.675µs

> Transactions per second (TPS): 50825.32

> Average time to execute transactions without running assertions: 19.657425ms

> Transactions per second (TPS) without running assertions: 132265.54

> Difference in time between running assertions and not running assertions: 31.498183ms
```

For consistency, the benchmark is ran 100 times against the same bundle of transactions.

The following output shows:

- How many transactions and assertions were ran;
- The average time it took to execute a transaction and associated assertion (`revm` + `phEVM`);
- The average TPS for exeucting transactions + assertions;
- The average time it takes to execute transactions without running any assertions (`revm` only);
- The TPS for executing transactions without running any assertions;
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
