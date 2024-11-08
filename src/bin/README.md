# `assertion-executor` benchmark

The follwing is a benchmark of the `assertion-executor`. It is a preliminary view of the performance and feature set expected for release.
It demonstrates:
- Executing regular transactions;
- Running assertions against them;
- Forking during assertion execution;
- Not finalizing transactions which fail assertions.

## Benchmark details

The benchmark simulates a real bundle validation scenario by using a similar gas load as to what one might expect in a production enviroment on a network like `Base`.

A bundle of transactions is ran, some of which trigger assertions, and the resulting output is displayed.

### Benchmark results

```bash
    =========================
    = Benchmarking complete =
    =========================

> 500 assertions ran against 500 transactions

> Average time elapsed in validating 500 transactions: 16.278586ms

> Average time per transaction: 32.557µs

> Transactions per second (TPS): 30715.20

> Average time to execute transactions without running assertions: 3.861258ms

> Transactions per second (TPS) without running assertions: 129491.48

> Difference in time between running assertions and not running assertions: 12.417328ms
```

For consistency, the benchmark is ran 100 times against the same bundle of transactions.

The following output shows:

- How many transactions and assertions were ran;
- The average time it took to execute a transaction and associated assertion;
- The average TPS for exeucting transactions + assertions;
- The average time it takes to execute transactions without running any assertions;
- The TPS for executing transactions without running any assertions;
- How much extra time it takes to run assertions compared to not running them.

## How to run

Nightly rust is required to run the benchmark. To run it execute the following:
```bash
cargo run --bin demo --profile maxperf
```

To guarantee compatibility, you can use the latest known good nightly with nix:
```bash
nix develop
cargo run --bin demo --profile maxperf
```
