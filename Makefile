build:
	cargo build --verbose

test:
	cargo test --verbose

format:
	cargo fmt --check

lint:
	cargo clippy  -- -D warnings

test-mocks:
	forge test --root contract-mocks -vvv

build-mocks:
	forge build --root contract-mocks

test-local:
	make build-mocks && make test
