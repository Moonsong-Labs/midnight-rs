# Development tasks for the midnight-rs workspace.
# Run `make` (or `make help`) to list targets. The CI workflow calls these
# same targets, so local and CI stay in sync.

CARGO ?= cargo

# Compiling contracts needs a compactc with the --analyzed-ir flag, which
# writes the analyzed-ir.sexp artifact the SDK consumes. The submodule pins
# upstream main plus that flag and builds with Nix; `make build-compactc`
# fetches + builds it. Override COMPACTC to use your own.
COMPACT_FORK := tools/compact-compiler
COMPACTC     ?= $(COMPACT_FORK)/result/bin/compactc

DEVNET_COMPOSE := devnet/docker-compose.yml
NODE_HEALTH    := http://localhost:9944/health
NODE_RPC       := http://127.0.0.1:9944
NODE_WS        := ws://127.0.0.1:9944
INDEXER_URL    := http://127.0.0.1:8088
INDEXER_GQL    := $(INDEXER_URL)/api/v4/graphql
DEV_SEED       := 0000000000000000000000000000000000000000000000000000000000000001
NODE_CONTAINER := midnight-example-node

# Examples that run against the devnet with no extra env (deploy + call).
# shielded-transfer / wallet-sync get their devnet env from dedicated targets.
EXAMPLES  := counter private-state contract-maintenance combine-and-sponsor shielded-swap
CONTRACTS := counter secret-counter unshielded-payout

# Interpreter test fixtures (crates/midnight-contract/tests/fixtures/<name>/).
# Each one carries its source `.compact` alongside the regenerated
# `compiler/analyzed-ir.sexp`; `regen-test-fixtures` re-emits it with
# the pinned compactc so the diff is reproducible.
TEST_FIXTURES := bboard counter election tiny
TEST_FIXTURE_DIR := crates/midnight-contract/tests/fixtures

# Conformance corpus (tests/conformance/fixtures/<name>/). Each fixture
# carries its source `.compact` plus the two compiler outputs both executors
# consume: `compiler/analyzed-ir.sexp` (Rust IR interpreter) and
# `contract/index.js` (TS codegen run by the ts-driver against the canonical
# @midnight-ntwrk/compact-runtime).
CONFORMANCE_FIXTURES := bboard containers counter kernel loops ops scopes shadowing slices \
                        structs tiny trees vectors
CONFORMANCE_DIR := tests/conformance
# The runtime tarball the driver installs. Generated, not committed: only the
# driver reads it, and anyone running the driver has just built the compiler.
# The name carries no version, so package.json never moves with the runtime.
COMPACT_RUNTIME_TGZ := ts-driver/vendor/compact-runtime.tgz

.PHONY: help fmt fmt-check clippy doc check test build audit ci \
        dev-up dev-wait dev-settle dev-down dev-status dev-logs \
        clippy-next-ledger test-next-ledger \
        test-e2e test-e2e-node-restart examples e2e run-shielded-transfer run-wallet-sync \
        build-compactc compile-contracts regen-test-fixtures \
        conformance conformance-regen regen-conformance-fixtures \
        vendor-compact-runtime

help:
	@echo "midnight-rs make targets:"
	@echo ""
	@echo "  Lint / build / test (no infra)"
	@echo "    fmt           cargo fmt --all"
	@echo "    fmt-check     cargo fmt --all --check"
	@echo "    clippy        cargo clippy --workspace --all-targets -- -D warnings"
	@echo "    doc           cargo doc --workspace --no-deps (RUSTDOCFLAGS=-D warnings)"
	@echo "    check         cargo check --workspace"
	@echo "    test          cargo test --workspace"
	@echo "    build         cargo build --workspace"
	@echo "    audit         cargo audit (fails on vulnerabilities; warnings allowed)"
	@echo "    ci            fmt-check + clippy + doc + check + test + audit (the CI gates)"
	@echo ""
	@echo "  Devnet (node + indexer via $(DEVNET_COMPOSE))"
	@echo "    dev-up        start the devnet and wait until it is ready"
	@echo "    dev-settle    wait until the indexer has every block the node calls best"
	@echo "    dev-down      stop the devnet"
	@echo "    dev-status    show container status"
	@echo "    dev-logs      follow devnet logs"
	@echo ""
	@echo "  Against a running devnet ('make dev-up' first)"
	@echo "    test-e2e      run the devnet integration tests"
	@echo "    test-e2e-node-restart  restart the node under a live provider (run alone, last)"
	@echo "    run-<name>    run one example (e.g. make run-counter)"
	@echo "    examples      run $(EXAMPLES)"
	@echo "    e2e           dev-up, run those examples, dev-down"
	@echo ""
	@echo "  Contracts (extended Compact compiler)"
	@echo "    build-compactc      fetch + build the compiler submodule (needs Nix)"
	@echo "    conformance         run the interpreter-vs-TS-runtime conformance gate"
	@echo "    conformance-regen   regenerate conformance goldens with the TS driver (needs Node)"
	@echo "    compile-contracts   recompile devnet/contracts/* with it"
	@echo "    regen-test-fixtures recompile $(TEST_FIXTURE_DIR)/*/analyzed-ir.sexp"
	@echo "    vendor-compact-runtime  rebuild the driver's vendored compact-runtime from the submodule"

# ============================================================
# Lint / build / test  (mirrors .github/workflows/ci.yml)
# ============================================================

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Rustdoc's own lints: a link to an item that moved or was renamed, a public
# doc pointing at a private item, a bare URL. `cargo check` and `cargo clippy`
# see none of them, so without this gate they only surface for whoever next
# builds the docs.
doc:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps

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

ci: fmt-check clippy doc check test audit
	@echo "OK: local CI gates passed"

# ============================================================
# Devnet (node + indexer)
# ============================================================

dev-up:
	docker compose -f $(DEVNET_COMPOSE) up -d
	@$(MAKE) --no-print-directory dev-wait

# Waits for a block past genesis, not merely for the indexer to answer. A dev
# devnet's genesis carries a `tblock` from months before wall clock, so a
# transaction built while only genesis exists gets an `intent.ttl` that is
# already in the past once block 1 lands, and the node rejects it with chain
# custom error 228.
dev-wait:
	@echo "Waiting for node..."
	@for _ in $$(seq 1 30); do curl -sf $(NODE_HEALTH) >/dev/null 2>&1 && break; sleep 2; done
	@echo "Waiting for the indexer to serve a block past genesis..."
	@for _ in $$(seq 1 60); do \
		height=$$(curl -sf $(INDEXER_GQL) -H 'Content-Type: application/json' \
			-d '{"query":"{ block { height } }"}' 2>/dev/null \
			| sed -n 's/.*"height":\([0-9][0-9]*\).*/\1/p'); \
		if [ -n "$$height" ] && [ "$$height" -ge 1 ]; then \
			echo "Devnet ready (height $$height)."; exit 0; \
		fi; \
		sleep 2; \
	done; \
	echo "ERROR: devnet did not reach a block past genesis"; \
	docker compose -f $(DEVNET_COMPOSE) logs; \
	exit 1

# Waits until the indexer has every block the node calls best. The indexer
# serves finalized blocks, two or three behind best, so a wallet synced from
# it cannot see a spend that still sits in a best block. A process that
# builds right after another one spent from the same seed draws the same
# Dust, and the node rejects it with chain custom error 196
# (DustDoubleSpend). Run this between two such processes.
dev-settle:
	@best=$$(curl -sf $(NODE_RPC) -H 'Content-Type: application/json' \
		-d '{"jsonrpc":"2.0","id":1,"method":"chain_getHeader","params":[]}' \
		| sed -n 's/.*"number":"0x\([0-9a-f]*\)".*/\1/p'); \
	if [ -z "$$best" ]; then echo "ERROR: node did not report a best block"; exit 1; fi; \
	best=$$((0x$$best)); \
	echo "Waiting for the indexer to reach the node's best block $$best..."; \
	for _ in $$(seq 1 60); do \
		height=$$(curl -sf $(INDEXER_GQL) -H 'Content-Type: application/json' \
			-d '{"query":"{ block { height } }"}' 2>/dev/null \
			| sed -n 's/.*"height":\([0-9][0-9]*\).*/\1/p'); \
		if [ -n "$$height" ] && [ "$$height" -ge "$$best" ]; then \
			echo "Indexer at height $$height."; exit 0; \
		fi; \
		sleep 2; \
	done; \
	echo "ERROR: indexer did not reach block $$best"; exit 1

dev-down:
	docker compose -f $(DEVNET_COMPOSE) down

dev-status:
	docker compose -f $(DEVNET_COMPOSE) ps

dev-logs:
	docker compose -f $(DEVNET_COMPOSE) logs -f

# The default build targets the deployed networks, which run ledger 8. These
# two targets build the newer generation, which the testnets take first. Only
# `midnight-types` names a generation, so they are what keep the rest of the
# workspace from drifting back into naming one.
NEXT_LEDGER_FEATURES := midnight-core/ledger-9,midnight-contract/ledger-9,conformance/ledger-9

clippy-next-ledger:
	cargo clippy --workspace --all-targets --features $(NEXT_LEDGER_FEATURES) -- -D warnings

test-next-ledger:
	cargo test --workspace --features $(NEXT_LEDGER_FEATURES)

# ============================================================
# Against a running devnet
# ============================================================

E2E_ENV := MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_E2E=1

# devnet/docker-compose.yml runs ledger 9, and the default build targets the
# deployed networks, which run ledger 8. Every target below that talks to the
# devnet asks for the newer generation. `midnight-indexer-client` names no
# generation, so its devnet test needs no feature.
L9 := --features ledger-9

# The devnet integration tests.
test-e2e:
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test node_e2e -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-wallet $(L9) --test integration -- --show-output --test-threads=1
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test balance_bare_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test prove_once_per_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test dust_registration_offer -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test dust_registration_submit -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test transaction_hash_identity -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test recover_unencrypted_mint -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test proving_outside_the_wallet_lock -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test unshielded_payout_to_user -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-indexer-client --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider $(L9) --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test mint_external_recipient -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract $(L9) --test e2e_contracts -- --show-output --test-threads=1

# Restarts the node container, so it disrupts every other test talking to it.
# Kept out of test-e2e; run it last, on its own.
test-e2e-node-restart:
	$(E2E_ENV) MIDNIGHT_NODE_CONTAINER=$(NODE_CONTAINER) \
		$(CARGO) test -p midnight-provider $(L9) --test devnet -- --ignored --show-output

# shielded-transfer and wallet-sync need devnet env; these explicit targets set
# it (and override the run-% pattern below).
run-shielded-transfer:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_NETWORK=undeployed \
		$(CARGO) run -p example-shielded-transfer $(L9)

run-wallet-sync:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_NETWORK=undeployed \
		MIDNIGHT_WALLET_SEED=$(DEV_SEED) $(CARGO) run -p example-wallet-sync $(L9)

# Run any other example: `make run-counter`, `make run-private-state`, ...
run-%:
	$(CARGO) run -p example-$* $(L9)

examples:
	@for ex in $(EXAMPLES); do \
		echo "=== example-$$ex ==="; \
		$(MAKE) --no-print-directory dev-settle || exit 1; \
		$(CARGO) run -p example-$$ex $(L9) || exit 1; \
	done

e2e: dev-up
	@$(MAKE) --no-print-directory examples
	@$(MAKE) --no-print-directory dev-down

# ============================================================
# Contracts (Compact — needs the extended compiler)
# ============================================================

# Resolve $(COMPACTC) to an absolute path, and refuse a build older than the
# submodule pin. `--analyzed-ir` is the flag the fork adds and every target
# below passes; a compiler without it answers with a bare `Usage: compactc`
# line that names neither the flag nor the fix.
define resolve-compactc
cc="$$(command -v $(COMPACTC) 2>/dev/null)"; \
if [ -z "$$cc" ]; then \
	echo "compactc not found ('$(COMPACTC)'). Run 'make build-compactc' (needs Nix), or set COMPACTC=<path>."; \
	exit 1; \
fi; \
case "$$cc" in /*) ;; *) cc="$(CURDIR)/$$cc" ;; esac; \
if ! "$$cc" --help 2>/dev/null | grep -q -- --analyzed-ir; then \
	echo "compactc at '$$cc' (version $$("$$cc" --version 2>/dev/null)) does not take --analyzed-ir."; \
	echo "The build is older than the $(COMPACT_FORK) pin. Run 'make build-compactc' to rebuild it."; \
	exit 1; \
fi
endef

# Fetch and build the extended Compact compiler from the submodule (needs Nix).
# Produces $(COMPACTC) (and the bundled zkir).
build-compactc:
	git submodule update --init --force $(COMPACT_FORK)
	cd $(COMPACT_FORK) && nix --extra-experimental-features 'nix-command flakes' build
	@echo "OK: compactc built at $(COMPACTC)"

# Recompile each contract and arrange the output into the layout the bindgen
# macro expects (top-level analyzed-ir.sexp + keys/ + zkir/). The compiler
# writes it under compiled/compiler/ and also emits a TS contract/ dir; we
# keep only what the SDK reads.
compile-contracts:
	@$(resolve-compactc); \
	for c in $(CONTRACTS); do \
		dir="devnet/contracts/$$c"; \
		echo "Compiling $$dir ..."; \
		( cd "$$dir" && \
			rm -rf compiled.tmp && \
			"$$cc" --analyzed-ir *.compact compiled.tmp && \
			rm -rf compiled && mkdir compiled && \
			mv compiled.tmp/compiler/analyzed-ir.sexp compiled/ && \
			mv compiled.tmp/keys compiled.tmp/zkir compiled/ && \
			rm -rf compiled.tmp ) || exit 1; \
	done; \
	echo "OK: contracts compiled"

# Recompile the interpreter test fixtures with the pinned compactc. Each
# fixture lives at $(TEST_FIXTURE_DIR)/<name>/ and carries both the source
# `<name>.compact` and the regenerated `compiler/analyzed-ir.sexp`. Only the
# JSON is consumed by the SDK tests, but the source travels with it so a
# regeneration is reproducible from inside the repo.
regen-test-fixtures:
	@$(resolve-compactc); \
	for f in $(TEST_FIXTURES); do \
		dir="$(TEST_FIXTURE_DIR)/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		"$$cc" --skip-zk --analyzed-ir "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiler/analyzed-ir.sexp"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: test fixtures regenerated"

# Rebuild the runtime tarball the driver installs, from the compiler submodule
# (needs Nix and Node). The runtime the pinned compactc targets is not
# published to npm, so the driver runs the one the submodule builds. The
# package's own build scripts need the compiler toolchain, which `npm pack`
# cannot run here, so the packed copy drops them.
vendor-compact-runtime:
	@out="$$(cd $(COMPACT_FORK) && nix --extra-experimental-features 'nix-command flakes' \
		build --no-link --print-out-paths '.#runtime.forPublish')"; \
	src="$$out/lib/node_modules/@midnight-ntwrk/compact-runtime"; \
	tmp="$$(mktemp -d)"; \
	cp -R "$$src/dist" "$$src/package.json" "$$src/README.md" "$$tmp/"; \
	chmod -R u+w "$$tmp"; \
	( cd "$$tmp" && node -e 'const fs = require("fs"); const p = "package.json"; const j = JSON.parse(fs.readFileSync(p)); delete j.scripts; delete j.devDependencies; fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n")' ); \
	version="$$(node -p "require(\"$$tmp/package.json\").version")"; \
	packed="$$(cd "$$tmp" && npm pack --silent --pack-destination "$$tmp")"; \
	dest="$(CURDIR)/$(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)"; \
	mkdir -p "$$(dirname "$$dest")"; \
	mv "$$tmp/$$packed" "$$dest"; \
	rm -rf "$$tmp"; \
	echo "OK: compact-runtime $$version vendored (now run 'make regen-conformance-fixtures')"

# Run the conformance gate: the Rust IR interpreter against the goldens
# emitted by the canonical TS runtime (already part of `make test`; this
# target is the focused loop).
conformance:
	$(CARGO) test -p conformance

# Regenerate the conformance goldens by running the corpus through the
# canonical @midnight-ntwrk/compact-runtime (needs Node 22+). The goldens are
# committed; `cargo test -p conformance` diffs the interpreter against them,
# and the codegen-drift workflow re-derives them to prove they still follow
# the compiler.
# NB: the generated contract/index.js and the vendored runtime are a matched
# pair. compactc writes its own --runtime-version into every index.js and the
# runtime refuses a mismatched minor, so a compiler bump means
# `vendor-compact-runtime` first, then this, then the driver's own API drift.
conformance-regen:
	@if [ ! -f "$(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)" ]; then \
		echo "no runtime tarball at $(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)."; \
		echo "It is generated, not committed. Run 'make vendor-compact-runtime' (needs Nix)."; \
		exit 1; \
	fi; \
	for f in $(CONFORMANCE_FIXTURES); do \
		if [ ! -f "$(CONFORMANCE_DIR)/fixtures/$$f/contract/index.js" ]; then \
			echo "no codegen for fixture '$$f'. It is generated, not committed."; \
			echo "Run 'make regen-conformance-fixtures' (needs Nix)."; \
			exit 1; \
		fi; \
	done
	cd $(CONFORMANCE_DIR) && npm ci && node ts-driver/driver.mjs

# Recompile the conformance corpus with the pinned compactc, refreshing both
# compiler outputs each fixture carries. Run `conformance-regen` afterwards:
# new codegen means new goldens.
regen-conformance-fixtures:
	@$(resolve-compactc); \
	for f in $(CONFORMANCE_FIXTURES); do \
		dir="$(CONFORMANCE_DIR)/fixtures/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		"$$cc" --skip-zk --analyzed-ir "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler" "$$dir/contract"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiler/analyzed-ir.sexp"; \
		mv "$$dir/compiled.tmp/contract/index.js" "$$dir/contract/index.js"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: conformance fixtures regenerated (now run 'make conformance-regen')"
