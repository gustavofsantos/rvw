# Pinned here and in .github/workflows/ci.yml. Installed under target/, so it
# never touches ~/.cargo/bin.
CARGO_AUDIT_VERSION := 0.22.2
CARGO_AUDIT := target/tools/bin/cargo-audit

.PHONY: build install install-gui uninstall test lint fmt audit check hooks clean

# PREFIX is where `make install` puts bin/rvw. Unset, cargo's own install root
# applies: $CARGO_INSTALL_ROOT, else ~/.cargo, so ~/.cargo/bin/rvw.
PREFIX ?=
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo "(devel)")

build:
	RVW_VERSION='$(VERSION)' cargo build --release -p rvw-cli

# An optimized build, installed (or upgraded in place) with cargo.
install:
	RVW_VERSION='$(VERSION)' cargo install --path crates/rvw-cli --locked --force $(if $(PREFIX),--root '$(PREFIX)')

# The desktop app. GUI_ROOT/bin/rvw-gui is installed with cargo either way.
# On Linux, its icon and a desktop entry whose Exec is the absolute path of the
# installed binary, as install.sh does. On macOS, an app bundle, rvw.app, under
# APPDIR.
DATADIR ?= $(or $(XDG_DATA_HOME),$(HOME)/.local/share)
APPDIR ?= $(HOME)/Applications
GUI_ROOT := $(or $(PREFIX),$(CARGO_INSTALL_ROOT),$(HOME)/.cargo)
APP := $(APPDIR)/rvw.app

install-gui:
	cargo install --path crates/rvw-gui --locked --force --root '$(GUI_ROOT)'
ifeq ($(shell uname -s),Darwin)
	rm -rf '$(APP)'
	mkdir -p '$(APP)/Contents/MacOS' '$(APP)/Contents/Resources'
	cp packaging/macos/Info.plist '$(APP)/Contents/Info.plist'
	cp packaging/macos/rvw.icns '$(APP)/Contents/Resources/rvw.icns'
	cp '$(GUI_ROOT)/bin/rvw-gui' '$(APP)/Contents/MacOS/rvw-gui'
	codesign --force --sign - '$(APP)'
else
	install -Dm644 packaging/linux/rvw.svg '$(DATADIR)/icons/hicolor/scalable/apps/rvw.svg'
	mkdir -p '$(DATADIR)/applications'
	sed 's|^Exec=.*|Exec=$(GUI_ROOT)/bin/rvw-gui|' packaging/linux/rvw.desktop >'$(DATADIR)/applications/rvw.desktop'
endif

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
