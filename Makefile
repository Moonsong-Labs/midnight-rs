# Development tasks for the midnight-rs workspace.
# Run `make` (or `make help`) to list targets.

CARGO ?= cargo

# The contracts under devnet/contracts/ need the extended Compact compiler
# (the contract-info-extensions fork — the stock compactc doesn't emit the `ir`
# field the bindgen macro requires). Override COMPACTC to point at it:
#   make compile-contracts COMPACTC=/path/to/fork/result/bin/compactc
COMPACTC ?= compactc

DEVNET_COMPOSE := devnet/docker-compose.yml
NODE_HEALTH    := http://localhost:9944/health
INDEXER_GQL    := http://127.0.0.1:8088/api/v3/graphql

# Examples that run against the devnet with no extra env (deploy + call).
# shielded-transfer / wallet-sync need env vars — see their READMEs.
EXAMPLES  := counter private-state contract-maintenance
CONTRACTS := counter secret-counter

.PHONY: help fmt fmt-check clippy check test build ci \
        dev-up dev-wait dev-down dev-status dev-logs \
        examples e2e compile-contracts

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
	@echo "    ci            fmt-check + clippy + check + test (the CI gates)"
	@echo ""
	@echo "  Devnet (node + indexer via $(DEVNET_COMPOSE))"
	@echo "    dev-up        start the devnet and wait until it is ready"
	@echo "    dev-down      stop the devnet"
	@echo "    dev-status    show container status"
	@echo "    dev-logs      follow devnet logs"
	@echo ""
	@echo "  Examples (need a running devnet: 'make dev-up' first)"
	@echo "    run-<name>    cargo run -p example-<name>  (e.g. make run-counter)"
	@echo "    examples      run $(EXAMPLES)"
	@echo "    e2e           dev-up, run those examples, dev-down"
	@echo ""
	@echo "  Contracts"
	@echo "    compile-contracts   recompile devnet/contracts/* (needs the extended COMPACTC)"

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

ci: fmt-check clippy check test
	@echo "OK: local CI gates passed"

# ============================================================
# Devnet (node + indexer)
# ============================================================

dev-up:
	docker compose -f $(DEVNET_COMPOSE) up -d
	@$(MAKE) --no-print-directory dev-wait

dev-wait:
	@echo "Waiting for node..."
	@until curl -sf $(NODE_HEALTH) >/dev/null 2>&1; do sleep 2; done
	@echo "Waiting for indexer..."
	@until curl -sf $(INDEXER_GQL) -H 'Content-Type: application/json' \
		-d '{"query":"{ block { height } }"}' 2>/dev/null | grep -q data; do sleep 2; done
	@echo "Devnet ready."

dev-down:
	docker compose -f $(DEVNET_COMPOSE) down

dev-status:
	docker compose -f $(DEVNET_COMPOSE) ps

dev-logs:
	docker compose -f $(DEVNET_COMPOSE) logs -f

# ============================================================
# Examples (require a running devnet)
# ============================================================

# Run any example: `make run-counter`, `make run-private-state`, ...
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

# Recompile each contract and arrange the output into the layout the bindgen
# macro expects (top-level contract-info.json + keys/ + zkir/). The fork writes
# contract-info.json under compiled/compiler/ and also emits a TS contract/ dir;
# we keep only what the SDK reads.
compile-contracts:
	@for c in $(CONTRACTS); do \
		dir=devnet/contracts/$$c; \
		echo "Compiling $$dir ..."; \
		( cd $$dir && \
			rm -rf compiled.tmp && \
			$(COMPACTC) *.compact compiled.tmp && \
			rm -rf compiled && mkdir compiled && \
			mv compiled.tmp/compiler/contract-info.json compiled/ && \
			mv compiled.tmp/keys compiled.tmp/zkir compiled/ && \
			rm -rf compiled.tmp ) || exit 1; \
	done
	@echo "OK: contracts compiled"
