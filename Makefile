build:
	cargo build --verbose

test:
	cargo test --verbose

format:
	cargo fmt --check

lint:
	cargo clippy  -- -D warnings