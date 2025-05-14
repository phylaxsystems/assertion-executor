# Assertion Executor

This crate gives you all the tools necessary to execute assertions.

## Installation

The easiest way to get started is to add the `assertion-executor` crate with the default features from the command-line using Cargo.

The default feature set executes for optimism.

```sh
cargo add assertion-executor --git ssh://git@github.com/phylaxsystems/assertion-executor.git 
```

Alternatively, you can add the following to your `Cargo.toml` file:

```toml
assertion-executor = { git = "ssh://git@github.com/phylaxsystems/assertion-executor.git", version = "0.1.0" }
```
Features List
* optimism - Supports optimism functionality.
* test - Provides several test utilities and convenience methods. 

## Overview

This repository contains the following primary components:

- [`assertion-executor`]: This component is used for executing assertions against a provided state. 
- [`assertion-store`] - Store for assertions that will return assertions matching triggers collected from transaction execution.
- [`assertion-indexer`] - Indexes state oracle contracts, fetches assertion from da, and extracts the assertion contract details from the bytecode. Once these events are finalized the assertions are moved to the store.
- [`overlay-db`] - Database with a cacheing layer for a `RethDatabase`
- [`fork-db`] -  Database for representing state using an underlying `revm::Database` and a overlay of the state differences.
- [`phevm`] - Provides cheatcodes for assertion execution, using an Inspector.

[`assertion-executor`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/executor/mod.rs
[`assertion-store`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/store/assertion_store.rs
[`assertion-indexer`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/store/indexer.rs
[`overlay-db`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/db/overlay/mod.rs
[`fork-db`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/db/fork_db.rs
[`phevm`]: https://github.com/phylaxsystems/assertion-executor/blob/main/src/inspectors/phevm.rs

## Supported Rust Versions (MSRV)
Currently this library requires nightly rust. 