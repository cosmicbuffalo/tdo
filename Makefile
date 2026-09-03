# Makefile for tdo — a modal, Git-backed kanban board for the terminal
#
# Common targets:
#   make install    build and install the tdo binary (reinstalls over any existing one)
#   make build      build a release binary into ./target/release
#   make run        run the TUI without installing
#   make check      fmt check + clippy + tests (what CI runs)
#   make test       run the test suite
#   make fmt        format the code in place
#   make lint       run clippy with warnings denied
#   make clean      remove build artifacts
#   make uninstall  remove the installed tdo binary

CARGO ?= cargo

.PHONY: all install uninstall build run check test fmt fmt-check lint clean

all: build

# Install (or reinstall) the binary. --force lets `make install` overwrite an
# existing install so it doubles as "reinstall".
install:
	$(CARGO) install --path . --force

uninstall:
	$(CARGO) uninstall tdo

build:
	$(CARGO) build --release

run:
	$(CARGO) run

# Mirror the Development section of the README.
check: fmt-check lint test

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --all-targets --all-features -- -D warnings

clean:
	$(CARGO) clean
