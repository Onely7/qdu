.PHONY: build test lint format wheel install bench hooks secrets clean

PREFIX ?= $(HOME)/.local
TARGET_DIR ?= $(or $(CARGO_TARGET_DIR),target)
MATURIN ?= uvx maturin

build:
	cargo build --release --locked

test:
	cargo test --locked

lint:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets -- -D warnings

format:
	cargo fmt --all

# The PyPI package: a wheel that installs the qdu binary.
wheel:
	$(MATURIN) build --release --locked --out dist

install: build
	install -d "$(PREFIX)/bin"
	install -m 755 "$(TARGET_DIR)/release/qdu" "$(PREFIX)/bin/qdu"
	@printf 'Installed qdu to %s\n' "$(PREFIX)/bin/qdu"

bench: build
	tools/benchmark.sh

hooks:
	pre-commit install

secrets:
	pre-commit run gitleaks-history --hook-stage manual --all-files

clean:
	cargo clean
	rm -rf dist
