.PHONY: build install uninstall test lint fmt audit check hooks clean

build:
	cargo build --release -p rvw-cli

install:
	cargo install --path crates/rvw-cli --locked

uninstall:
	cargo uninstall rvw-cli

test:
	cargo test --workspace
	bats test/rvw.bats

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all --check

fmt:
	cargo fmt --all

# Needs cargo-audit: cargo install cargo-audit --locked
audit:
	cargo audit

check: lint audit test

hooks:
	git config core.hooksPath .githooks

clean:
	cargo clean
