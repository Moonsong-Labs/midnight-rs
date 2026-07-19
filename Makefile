# Development tasks for the midnight-rs workspace.
# Run `make` (or `make help`) to list targets. The CI workflow calls these
# same targets, so local and CI stay in sync.

CARGO ?= cargo

# The contracts under devnet/contracts/ need the extended Compact compiler (the
# contract-info-extensions fork — the stock compactc doesn't emit the `ir` field
# the bindgen macro requires). It's a git submodule that builds with Nix;
# `make build-compactc` fetches + builds it. Override COMPACTC to use your own.
COMPACT_FORK := tools/compact-compiler
COMPACTC     ?= $(COMPACT_FORK)/result/bin/compactc

DEVNET_COMPOSE := devnet/docker-compose.yml
NODE_HEALTH    := http://localhost:9944/health
NODE_WS        := ws://127.0.0.1:9944
INDEXER_URL    := http://127.0.0.1:8088
INDEXER_GQL    := $(INDEXER_URL)/api/v3/graphql
DEV_SEED       := 0000000000000000000000000000000000000000000000000000000000000001

# Examples that run against the devnet with no extra env (deploy + call).
# shielded-transfer / wallet-sync get their devnet env from dedicated targets.
EXAMPLES  := counter private-state contract-maintenance combine-and-sponsor
CONTRACTS := counter secret-counter

# Interpreter test fixtures (crates/midnight-contract/tests/fixtures/<name>/).
# Each one carries its source `.compact` alongside the regenerated
# `compiler/contract-info.json`; `regen-test-fixtures` re-emits the JSON with
# the pinned compactc so the diff is reproducible.
TEST_FIXTURES := bboard counter election tiny
TEST_FIXTURE_DIR := crates/midnight-contract/tests/fixtures

# Conformance corpus (tests/conformance/fixtures/<name>/). Each fixture
# carries its source `.compact` plus the two compiler outputs both executors
# consume: `compiler/contract-info.json` (Rust IR interpreter) and
# `contract/index.js` (TS codegen run by the ts-driver against the canonical
# @midnight-ntwrk/compact-runtime).
CONFORMANCE_FIXTURES := bboard counter ops tiny
CONFORMANCE_DIR := tests/conformance

.PHONY: help fmt fmt-check clippy check test build audit ci \
        dev-up dev-wait dev-down dev-status dev-logs \
        test-e2e examples e2e run-shielded-transfer run-wallet-sync \
        build-compactc compile-contracts regen-test-fixtures \
        conformance conformance-regen regen-conformance-fixtures

help:
	@echo "midnight-rs make targets:"
	@echo ""
	@echo "  Lint / build / test (no infra)"
	@echo "    fmt           cargo fmt --all"
	@echo "    fmt-check     cargo fmt --all --check"
	@echo "    clippy        cargo clippy --workspace --all-targets -- -D warnings"
	@echo "    check         cargo check --workspace"
	@echo "    test          cargo test --workspace"
	@echo "    build         cargo build --workspace"
	@echo "    audit         cargo audit (fails on vulnerabilities; warnings allowed)"
	@echo "    ci            fmt-check + clippy + check + test + audit (the CI gates)"
	@echo ""
	@echo "  Devnet (node + indexer via $(DEVNET_COMPOSE))"
	@echo "    dev-up        start the devnet and wait until it is ready"
	@echo "    dev-down      stop the devnet"
	@echo "    dev-status    show container status"
	@echo "    dev-logs      follow devnet logs"
	@echo ""
	@echo "  Against a running devnet ('make dev-up' first)"
	@echo "    test-e2e      run the devnet integration tests"
	@echo "    run-<name>    run one example (e.g. make run-counter)"
	@echo "    examples      run $(EXAMPLES)"
	@echo "    e2e           dev-up, run those examples, dev-down"
	@echo ""
	@echo "  Contracts (extended Compact compiler)"
	@echo "    build-compactc      fetch + build the compiler submodule (needs Nix)"
	@echo "    conformance         run the interpreter-vs-TS-runtime conformance gate"
	@echo "    conformance-regen   regenerate conformance goldens with the TS driver (needs Node)"
	@echo "    compile-contracts   recompile devnet/contracts/* with it"
	@echo "    regen-test-fixtures recompile $(TEST_FIXTURE_DIR)/*/contract-info.json"

# ============================================================
# Lint / build / test  (mirrors .github/workflows/ci.yml)
# ============================================================

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

check:
	$(CARGO) check --workspace

test:
	$(CARGO) test --workspace

build:
	$(CARGO) build --workspace

# cargo audit checks the lockfile against the RustSec advisory database.
# Exit code 0 means no vulnerabilities; the "unmaintained" warnings on
# transitive deps we don't control (paste, bincode, libsecp256k1,
# number_prefix) are allowed and do not fail the gate.
audit:
	$(CARGO) audit

ci: fmt-check clippy check test audit
	@echo "OK: local CI gates passed"

# ============================================================
# Devnet (node + indexer)
# ============================================================

dev-up:
	docker compose -f $(DEVNET_COMPOSE) up -d
	@$(MAKE) --no-print-directory dev-wait

dev-wait:
	@echo "Waiting for node..."
	@for _ in $$(seq 1 30); do curl -sf $(NODE_HEALTH) >/dev/null 2>&1 && break; sleep 2; done
	@echo "Waiting for indexer..."
	@for _ in $$(seq 1 30); do \
		if curl -sf $(INDEXER_GQL) -H 'Content-Type: application/json' \
			-d '{"query":"{ block { height } }"}' 2>/dev/null | grep -q data; then \
			echo "Devnet ready."; exit 0; \
		fi; \
		sleep 2; \
	done; \
	echo "ERROR: devnet did not become ready"; \
	docker compose -f $(DEVNET_COMPOSE) logs; \
	exit 1

dev-down:
	docker compose -f $(DEVNET_COMPOSE) down

dev-status:
	docker compose -f $(DEVNET_COMPOSE) ps

dev-logs:
	docker compose -f $(DEVNET_COMPOSE) logs -f

# ============================================================
# Against a running devnet
# ============================================================

# The devnet integration tests.
test-e2e:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_E2E=1 \
		$(CARGO) test --test node_e2e -- --show-output
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_E2E=1 \
		$(CARGO) test -p midnight-wallet --test integration -- --show-output --test-threads=1

# shielded-transfer and wallet-sync need devnet env; these explicit targets set
# it (and override the run-% pattern below).
run-shielded-transfer:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_NETWORK=undeployed \
		$(CARGO) run -p example-shielded-transfer

run-wallet-sync:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_NETWORK=undeployed \
		MIDNIGHT_WALLET_SEED=$(DEV_SEED) $(CARGO) run -p example-wallet-sync

# Run any other example: `make run-counter`, `make run-private-state`, ...
run-%:
	$(CARGO) run -p example-$*

examples:
	@for ex in $(EXAMPLES); do \
		echo "=== example-$$ex ==="; \
		$(CARGO) run -p example-$$ex || exit 1; \
	done

e2e: dev-up
	@$(MAKE) --no-print-directory examples
	@$(MAKE) --no-print-directory dev-down

# ============================================================
# Contracts (Compact — needs the extended compiler)
# ============================================================

# Fetch and build the extended Compact compiler from the submodule (needs Nix).
# Produces $(COMPACTC) (and the bundled zkir).
build-compactc:
	git submodule update --init --force $(COMPACT_FORK)
	cd $(COMPACT_FORK) && nix --extra-experimental-features 'nix-command flakes' build
	@echo "OK: compactc built at $(COMPACTC)"

# Recompile each contract and arrange the output into the layout the bindgen
# macro expects (top-level contract-info.json + keys/ + zkir/). The fork writes
# contract-info.json under compiled/compiler/ and also emits a TS contract/ dir;
# we keep only what the SDK reads.
compile-contracts:
	@cc="$$(command -v $(COMPACTC) 2>/dev/null)"; \
	if [ -z "$$cc" ]; then \
		echo "compactc not found ('$(COMPACTC)'). Run 'make build-compactc' (needs Nix), or set COMPACTC=<path>."; \
		exit 1; \
	fi; \
	case "$$cc" in /*) ;; *) cc="$(CURDIR)/$$cc" ;; esac; \
	for c in $(CONTRACTS); do \
		dir="devnet/contracts/$$c"; \
		echo "Compiling $$dir ..."; \
		( cd "$$dir" && \
			rm -rf compiled.tmp && \
			"$$cc" *.compact compiled.tmp && \
			rm -rf compiled && mkdir compiled && \
			mv compiled.tmp/compiler/contract-info.json compiled/ && \
			mv compiled.tmp/keys compiled.tmp/zkir compiled/ && \
			rm -rf compiled.tmp ) || exit 1; \
	done; \
	echo "OK: contracts compiled"

# Recompile the interpreter test fixtures with the pinned compactc. Each
# fixture lives at $(TEST_FIXTURE_DIR)/<name>/ and carries both the source
# `<name>.compact` and the regenerated `compiler/contract-info.json`. Only the
# JSON is consumed by the SDK tests, but the source travels with it so a
# regeneration is reproducible from inside the repo.
regen-test-fixtures:
	@cc="$$(command -v $(COMPACTC) 2>/dev/null)"; \
	if [ -z "$$cc" ]; then \
		echo "compactc not found ('$(COMPACTC)'). Run 'make build-compactc' (needs Nix), or set COMPACTC=<path>."; \
		exit 1; \
	fi; \
	case "$$cc" in /*) ;; *) cc="$(CURDIR)/$$cc" ;; esac; \
	for f in $(TEST_FIXTURES); do \
		dir="$(TEST_FIXTURE_DIR)/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		"$$cc" "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler"; \
		mv "$$dir/compiled.tmp/compiler/contract-info.json" "$$dir/compiler/contract-info.json"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: test fixtures regenerated"

# Run the conformance gate: the Rust IR interpreter against the goldens
# emitted by the canonical TS runtime (already part of `make test`; this
# target is the focused loop).
conformance:
	$(CARGO) test -p conformance

# Regenerate the conformance goldens by running the corpus through the
# canonical @midnight-ntwrk/compact-runtime (needs Node 22+). CI re-runs
# this and fails when expected/ drifts from what is committed.
conformance-regen:
	cd $(CONFORMANCE_DIR) && npm ci && node ts-driver/driver.mjs

# Recompile the conformance corpus with the pinned compactc, refreshing both
# compiler outputs each fixture carries. Run `conformance-regen` afterwards:
# new codegen means new goldens.
regen-conformance-fixtures:
	@cc="$$(command -v $(COMPACTC) 2>/dev/null)"; \
	if [ -z "$$cc" ]; then \
		echo "compactc not found ('$(COMPACTC)'). Run 'make build-compactc' (needs Nix), or set COMPACTC=<path>."; \
		exit 1; \
	fi; \
	case "$$cc" in /*) ;; *) cc="$(CURDIR)/$$cc" ;; esac; \
	for f in $(CONFORMANCE_FIXTURES); do \
		dir="$(CONFORMANCE_DIR)/fixtures/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		"$$cc" "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler" "$$dir/contract"; \
		mv "$$dir/compiled.tmp/compiler/contract-info.json" "$$dir/compiler/contract-info.json"; \
		mv "$$dir/compiled.tmp/contract/index.js" "$$dir/contract/index.js"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: conformance fixtures regenerated (now run 'make conformance-regen')"
