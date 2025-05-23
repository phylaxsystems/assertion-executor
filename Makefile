# Build the binary
build:
	cargo +nightly build  --release

# Build the contract mocks and run the rust tests
test:
	forge build --root contract-mocks && cargo +nightly test

# Build the contract mocks and run the rust tests using the optimism feature flag
test-optimism:
	forge build --root contract-mocks && cargo +nightly test --features optimism

# Validate formatting
format:
	cargo +nightly fmt --check

# Errors if there is a warning with clippy
lint: 
	cargo clippy --all-targets --workspace --locked  --profile dev -- -D warnings

# Run foundry tests against the contract mocks
test-mocks:
	forge test --root contract-mocks -vvv

# Can be used as a manual pre-commit check
pre-commit:
	cargo +nightly fmt && make lint
