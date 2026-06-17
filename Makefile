.PHONY: generate check fmt

generate:
	cargo run -p queryforge-cli -- generate queryforge.toml

check:
	cargo check --workspace --all-features

fmt:
	cargo fmt --all

test:
	cargo nextest run --workspace --all-features

clean:
	cargo clean
