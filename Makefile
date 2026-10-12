# Pinned here and in .github/workflows/ci.yml. Installed under target/, so it
# never touches ~/.cargo/bin.
CARGO_AUDIT_VERSION := 0.22.2
CARGO_AUDIT := target/tools/bin/cargo-audit

.PHONY: build install uninstall test lint fmt audit check hooks clean

# PREFIX is where `make install` puts bin/rvw. Unset, cargo's own install root
# applies: $CARGO_INSTALL_ROOT, else ~/.cargo, so ~/.cargo/bin/rvw.
PREFIX ?=
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo "(devel)")

build:
	RVW_VERSION='$(VERSION)' cargo build --release -p rvw-cli

# An optimized build, installed (or upgraded in place) with cargo.
install:
	RVW_VERSION='$(VERSION)' cargo install --path crates/rvw-cli --locked --force $(if $(PREFIX),--root '$(PREFIX)')

uninstall:
	cargo uninstall rvw-cli $(if $(PREFIX),--root '$(PREFIX)')

test:
	cargo test --workspace
	bats test/rvw.bats

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all --check

fmt:
	cargo fmt --all

$(CARGO_AUDIT):
	cargo install cargo-audit --locked --version $(CARGO_AUDIT_VERSION) --root target/tools

audit: $(CARGO_AUDIT)
	$(CARGO_AUDIT) audit

check: lint audit test

hooks:
	git config core.hooksPath .githooks

clean:
	cargo clean
