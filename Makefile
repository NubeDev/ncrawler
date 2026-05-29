# ncrawler — developer Makefile
#
# Thin wrapper around the cargo commands the SCOPE/WORKFLOW gates
# require. Run `make help` for the full list.

CARGO        ?= cargo
WORKSPACE    := --workspace
ALL_TARGETS  := --all-targets
DENY_WARN    := -- -D warnings

# Live-network / live-Chrome / live-Claude tests are gated on this env
# var per SCOPE (default OFF in CI; opt in locally).
LIVE         ?= 0
ifeq ($(LIVE),1)
TEST_ENV     := RUN_LIVE_TESTS=1
TEST_FLAGS   := -- --include-ignored
else
TEST_ENV     :=
TEST_FLAGS   :=
endif

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z0-9_.-]+:.*?## / {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# ---------------------------------------------------------------- build

.PHONY: build
build: ## cargo build --workspace
	$(CARGO) build $(WORKSPACE)

.PHONY: build-all
build-all: ## build with every optional feature (store-qdrant, ...)
	$(CARGO) build $(WORKSPACE) --features store-qdrant

.PHONY: release
release: ## cargo build --workspace --release
	$(CARGO) build $(WORKSPACE) --release

# ---------------------------------------------------------------- check

.PHONY: check
check: ## cargo check --workspace --all-targets
	$(CARGO) check $(WORKSPACE) $(ALL_TARGETS)

.PHONY: fmt
fmt: ## cargo fmt --all
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## cargo fmt --all -- --check (CI gate)
	$(CARGO) fmt --all -- --check

.PHONY: clippy
clippy: ## cargo clippy --workspace --all-targets -- -D warnings (CI gate)
	$(CARGO) clippy $(WORKSPACE) $(ALL_TARGETS) $(DENY_WARN)

# ---------------------------------------------------------------- test

.PHONY: test
test: ## cargo test --workspace (LIVE=1 to include #[ignore] live tests)
	$(TEST_ENV) $(CARGO) test $(WORKSPACE) $(TEST_FLAGS)

.PHONY: test-live
test-live: ## cargo test --workspace with RUN_LIVE_TESTS=1
	$(MAKE) test LIVE=1

# ---------------------------------------------------------------- gates

.PHONY: ci
ci: fmt-check clippy test ## full CI gate: fmt-check + clippy + test

.PHONY: pre-push
pre-push: fmt clippy test ## fix fmt, then run clippy + test before pushing

# ---------------------------------------------------------------- audit

.PHONY: tree
tree: ## cargo tree -e normal (verify single browser stack, no spider_chrome)
	$(CARGO) tree -e normal

.PHONY: tree-check
tree-check: ## fail if spider_chrome leaks into the normal dep graph
	@if $(CARGO) tree -e normal | grep -q spider_chrome; then \
		echo "ERROR: spider_chrome found in dep graph (SCOPE non-goal)"; \
		exit 1; \
	fi
	@echo "ok: no spider_chrome in normal dep graph"

.PHONY: deny
deny: ## cargo deny check (licenses, advisories) — requires cargo-deny
	$(CARGO) deny check || (echo "install: cargo install cargo-deny" && exit 1)

.PHONY: outdated
outdated: ## cargo outdated — requires cargo-outdated
	$(CARGO) outdated -R || (echo "install: cargo install cargo-outdated" && exit 1)

# ---------------------------------------------------------------- run

.PHONY: run
run: ## run the CLI (use ARGS="scrape grafana ...")
	$(CARGO) run -p ncrawler-cli -- $(ARGS)

.PHONY: ls
ls: ## ncrawler ls — list artifacts in the local store
	$(CARGO) run -p ncrawler-cli -- ls

# ---------------------------------------------------------------- clean

.PHONY: clean
clean: ## cargo clean
	$(CARGO) clean

.PHONY: clean-runs
clean-runs: ## remove the on-disk artifact store under ./runs
	rm -rf runs/
