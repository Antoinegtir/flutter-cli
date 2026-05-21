# flutter-cli — developer task runner
#
# Run `make` or `make help` to list targets.

CLI_CRATE := fl-cli
BIN       := flutter-cli
NPM_DIR   := npm

.DEFAULT_GOAL := help

# ---------------------------------------------------------------------------
# Build / install
# ---------------------------------------------------------------------------

.PHONY: install
install: ## Build in release and install the binary to ~/.cargo/bin (force)
	cargo install --path crates/$(CLI_CRATE) --force

.PHONY: uninstall
uninstall: ## Remove the installed binary from ~/.cargo/bin
	cargo uninstall $(CLI_CRATE)

.PHONY: build
build: ## Release build of the CLI binary
	cargo build --release -p $(CLI_CRATE)

.PHONY: build-all
build-all: ## Release build of the whole workspace
	cargo build --release --workspace

.PHONY: run
run: ## Run the CLI (forward args with ARGS=..., e.g. `make run ARGS=run`)
	cargo run -p $(CLI_CRATE) -- $(ARGS)

# ---------------------------------------------------------------------------
# Quality gates (mirror CI)
# ---------------------------------------------------------------------------

.PHONY: test
test: ## Run the full workspace test suite
	cargo test --workspace

.PHONY: fmt
fmt: ## Format all code in place
	cargo fmt --all

.PHONY: fmt-check
fmt-check: ## Check formatting without writing (CI mode)
	cargo fmt --all -- --check

.PHONY: clippy
clippy: ## Lint with clippy, warnings denied (CI mode)
	cargo clippy --workspace -- -D warnings

.PHONY: snapshots
snapshots: ## Re-generate insta UI snapshots, then run tests
	INSTA_UPDATE=always cargo test --workspace

.PHONY: check
check: fmt-check clippy test ## Run every CI gate locally (fmt + clippy + tests)

# ---------------------------------------------------------------------------
# Release / publish
# ---------------------------------------------------------------------------

.PHONY: tag
tag: ## Create + push a git tag from VERSION (e.g. `make tag VERSION=0.3.1`)
	@test -n "$(VERSION)" || { echo "Usage: make tag VERSION=X.Y.Z"; exit 1; }
	git tag v$(VERSION)
	git push origin v$(VERSION)

.PHONY: npm-publish
npm-publish:
	cd $(NPM_DIR) && npm publish --access public

# ---------------------------------------------------------------------------
# Housekeeping
# ---------------------------------------------------------------------------

.PHONY: clean
clean: ## Remove build artifacts
	cargo clean

.PHONY: help
help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
