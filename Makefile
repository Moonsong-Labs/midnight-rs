# Development tasks for the midnight-rs workspace.
# Run `make` (or `make help`) to list targets. The CI workflow calls these
# same targets, so local and CI stay in sync.

CARGO ?= cargo

# Compiling contracts needs the compactc of the fork RomarQ/compact, which adds
# the --analyzed-ir flag that writes the analyzed-ir.sexp artifact the SDK
# consumes. COMPACT_REV is the fork commit this repository tests, and a
# workflow on the fork publishes its image. Override COMPACTC to use your own
# build of the fork.
COMPACT_REV    := fa2181fbc6dac2135defdb4f55ce10d8332185d5
COMPACTC_IMAGE := ghcr.io/romarq/compactc:$(COMPACT_REV)
# The cache of public parameters that key generation reads and fills, in the
# fall-back order of the SDK prover, so the two share one cache. Set
# MIDNIGHT_PP to use another cache. Each target creates it before the run,
# because Docker creates a missing mount source as root, and the prover then
# cannot write to it.
ZK_PARAMS := $(or $(MIDNIGHT_PP),$(or $(XDG_CACHE_HOME),$(HOME)/.cache)/midnight/zk-params)
# The container sees only the repository, at the same path, and runs from its
# root, so the targets give paths relative to the root. The user mapping makes
# the caller the owner of the output files.
COMPACTC ?= docker run --rm --user $$(id -u):$$(id -g) -v "$(CURDIR):$(CURDIR)" -w "$(CURDIR)" \
	-v "$(ZK_PARAMS):/zk-params" -e MIDNIGHT_PP=/zk-params \
	--entrypoint compactc $(COMPACTC_IMAGE)

# The ledger generation the devnet runs: 8 (devnet/docker-compose.yml) or 9
# (devnet/docker-compose.ledger-9.yml). Both listen on the same ports, so run
# one at a time.
DEVNET_LEDGER ?= 8
ifeq ($(DEVNET_LEDGER),9)
DEVNET_COMPOSE := devnet/docker-compose.ledger-9.yml
NODE_CONTAINER := midnight-example-node-l9
else ifeq ($(DEVNET_LEDGER),8)
DEVNET_COMPOSE := devnet/docker-compose.yml
NODE_CONTAINER := midnight-example-node
else
$(error DEVNET_LEDGER must be 8 or 9, not '$(DEVNET_LEDGER)')
endif
NODE_HEALTH    := http://localhost:9944/health
NODE_RPC       := http://127.0.0.1:9944
NODE_WS        := ws://127.0.0.1:9944
INDEXER_URL    := http://127.0.0.1:8088
INDEXER_GQL    := $(INDEXER_URL)/api/v3/graphql
DEV_SEED       := 0000000000000000000000000000000000000000000000000000000000000001

# Examples that run against the devnet with no extra env (deploy + call).
# shielded-transfer / wallet-sync get their devnet env from dedicated targets.
EXAMPLES  := counter private-state contract-maintenance combine-and-sponsor shielded-swap
CONTRACTS := counter secret-counter shielded-mint unshielded-payout

# Interpreter test fixtures (crates/midnight-contract/tests/fixtures/<name>/).
# Each one carries its source `.compact` alongside the regenerated
# `compiler/analyzed-ir.sexp`; `regen-test-fixtures` re-emits it with
# the compactc at COMPACT_REV so the diff is reproducible.
TEST_FIXTURES := bboard counter election tiny
TEST_FIXTURE_DIR := crates/midnight-contract/tests/fixtures

# Conformance corpus (tests/conformance/fixtures/<name>/). Each fixture
# carries its source `.compact` plus the two compiler outputs both executors
# consume: `compiler/analyzed-ir.sexp` (Rust IR interpreter) and
# `contract/index.js` (TS codegen run by the ts-driver against the canonical
# @midnight-ntwrk/compact-runtime).
CONFORMANCE_FIXTURES := bboard containers counter defaults indexing kernel loops ops peers \
                        scopes shadowing slices structs tiny trees vectors
CONFORMANCE_DIR := tests/conformance
# The runtime tarball the driver installs. Generated, not committed: only the
# driver reads it, and `vendor-compact-runtime` builds it from COMPACT_REV.
# The name carries no version, so package.json never moves with the runtime.
COMPACT_RUNTIME_TGZ := ts-driver/vendor/compact-runtime.tgz

.PHONY: help fmt fmt-check clippy doc check test build audit ci \
        dev-up dev-wait dev-settle dev-down dev-status dev-logs \
        test-e2e test-e2e-node-restart examples e2e run-shielded-transfer run-wallet-sync \
        fork-up fork-upgrade fork-test fork-down \
        compile-contracts regen-test-fixtures \
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
	@echo "  Devnet (node + indexer via $(DEVNET_COMPOSE); DEVNET_LEDGER=9 for ledger 9)"
	@echo "    dev-up        start the devnet and wait until it is ready"
	@echo "    dev-settle    wait until the indexer has every block the node calls best"
	@echo "    dev-down      stop the devnet"
	@echo "    dev-status    show container state, restart counts and recent logs"
	@echo "    dev-logs      follow devnet logs"
	@echo ""
	@echo "  Fork devnet (starts on ledger 8 and forks to ledger 9; the devnet's ports)"
	@echo "    fork-up       write a ledger 8 chain spec and start the fork devnet"
	@echo "    fork-test     run the fork crossing test, which forks the chain (fork-upgrade)"
	@echo "    fork-down     stop the fork devnet"
	@echo ""
	@echo "  Against a running devnet ('make dev-up' first)"
	@echo "    test-e2e      run the devnet integration tests"
	@echo "    test-e2e-node-restart  restart the node under a live provider (run alone, last)"
	@echo "    run-<name>    run one example (e.g. make run-counter)"
	@echo "    examples      run $(EXAMPLES)"
	@echo "    e2e           dev-up, run those examples, dev-down"
	@echo ""
	@echo "  Contracts (compactc runs in $(COMPACTC_IMAGE))"
	@echo "    compile-contracts   recompile devnet/contracts/*, with keys (needs Docker)"
	@echo "    regen-test-fixtures recompile $(TEST_FIXTURE_DIR)/*/compiler/analyzed-ir.sexp (needs Docker)"
	@echo "    regen-conformance-fixtures  recompile the conformance corpus (needs Docker)"
	@echo "    conformance         run the interpreter-vs-TS-runtime conformance gate"
	@echo "    conformance-regen   regenerate conformance goldens with the TS driver (needs Node)"
	@echo "    vendor-compact-runtime  build the driver's compact-runtime at COMPACT_REV (needs Nix and Node)"

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
# custom error 182.
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
	-docker compose -f $(DEVNET_COMPOSE) ps -a
	-ids=$$(docker compose -f $(DEVNET_COMPOSE) ps -aq); [ -z "$$ids" ] || docker inspect -f '{{.Name}} restarts={{.RestartCount}} exit={{.State.ExitCode}} oom={{.State.OOMKilled}} started={{.State.StartedAt}} finished={{.State.FinishedAt}}' $$ids
	-docker compose -f $(DEVNET_COMPOSE) logs --no-color --timestamps --tail 500

dev-logs:
	docker compose -f $(DEVNET_COMPOSE) logs -f

# ============================================================
# Fork devnet: a chain that starts on ledger 8 and forks to ledger 9
# ============================================================

FORK_COMPOSE := devnet/fork/docker-compose.yml
# The ledger 8 release the chain starts from, and the toolkit of the ledger 9
# release it forks to (the node image devnet/fork/docker-compose.yml runs).
FORK_FROM_NODE := midnightntwrk/midnight-node:1.0.1
FORK_TO_NODE   := midnightntwrk/midnight-node:2.1.0-rc.3
FORK_TOOLKIT   := midnightntwrk/midnight-node-toolkit:2.1.0-rc.3
FORK_ARCH      := $(if $(filter arm64 aarch64,$(shell uname -m)),arm64,amd64)

fork-up:
	docker run --rm -e CFG_PRESET=dev $(FORK_FROM_NODE) build-spec > devnet/fork/chainspec.json
	docker compose -f $(FORK_COMPOSE) up -d
	@$(MAKE) --no-print-directory dev-wait DEVNET_COMPOSE=$(FORK_COMPOSE)

# Fork the chain: apply the ledger 9 runtime through the dev chain's
# governance, as the node's own fork test does.
fork-upgrade:
	docker run --rm --entrypoint cat $(FORK_TO_NODE) \
		/artifacts-$(FORK_ARCH)/midnight_node_runtime.compact.compressed.wasm > devnet/fork/runtime.wasm
	docker run --rm --network midnight-fork_default \
		-v $(CURDIR)/devnet/fork/runtime.wasm:/runtime.wasm:ro $(FORK_TOOLKIT) \
		runtime-upgrade --wasm-file /runtime.wasm -c //Dave -c //Eve -t //Alice -t //Bob \
		--rpc-url ws://node:9944 --signer-key //Alice

# The crossing test forks the chain itself, so it runs once per `fork-up`.
fork-test:
	$(E2E_ENV) MIDNIGHT_FORK_UPGRADE_CMD="$(MAKE) -C $(CURDIR) --no-print-directory fork-upgrade" \
		$(CARGO) test -p midnight-contract --test fork_crossing -- --show-output

fork-down:
	docker compose -f $(FORK_COMPOSE) down

# ============================================================
# Against a running devnet
# ============================================================

E2E_ENV := MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_E2E=1 \
	MIDNIGHT_LEDGER=$(DEVNET_LEDGER)

# The devnet integration tests.
test-e2e:
	$(E2E_ENV) $(CARGO) test --test node_e2e -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-wallet --test integration -- --show-output --test-threads=1
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test balance_bare_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test prove_once_per_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_registration_offer -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_registration_submit -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test transaction_hash_identity -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test recover_unencrypted_mint -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test proving_outside_the_wallet_lock -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test unshielded_payout_to_user -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-indexer-client --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test mint_external_recipient -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test e2e_contracts -- --show-output --test-threads=1

# Restarts the node container, so it disrupts every other test talking to it.
# Kept out of test-e2e; run it last, on its own.
test-e2e-node-restart:
	$(E2E_ENV) MIDNIGHT_NODE_CONTAINER=$(NODE_CONTAINER) \
		$(CARGO) test -p midnight-provider --test devnet -- --ignored --show-output

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
		$(MAKE) --no-print-directory dev-settle || exit 1; \
		$(CARGO) run -p example-$$ex || exit 1; \
	done

e2e: dev-up
	@$(MAKE) --no-print-directory examples
	@$(MAKE) --no-print-directory dev-down

# ============================================================
# Contracts (Compact, compiled in the compiler image)
# ============================================================

# Recompile each contract into its compiled/ directory, in the layout the
# compiler writes: compiler/analyzed-ir.sexp, keys/ and zkir/. The contract!
# macro reads the artifact, and with_zk_config takes compiled/. The SDK reads
# none of the compiler's other output, such as the TS contract/ directory, so
# the target drops it.
compile-contracts:
	@mkdir -p "$(ZK_PARAMS)"
	@for c in $(CONTRACTS); do \
		dir="devnet/contracts/$$c"; \
		echo "Compiling $$dir ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		$(COMPACTC) --analyzed-ir "$$dir"/*.compact "$$dir/compiled.tmp" || exit 1; \
		rm -rf "$$dir/compiled"; \
		mkdir -p "$$dir/compiled/compiler"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiled/compiler/"; \
		mv "$$dir/compiled.tmp/keys" "$$dir/compiled.tmp/zkir" "$$dir/compiled/"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: contracts compiled"

# Recompile the interpreter test fixtures with the compactc at COMPACT_REV.
# Each fixture lives at $(TEST_FIXTURE_DIR)/<name>/ and carries both the source
# `<name>.compact` and the regenerated `compiler/analyzed-ir.sexp`. Only the
# JSON is consumed by the SDK tests, but the source travels with it so a
# regeneration is reproducible from inside the repo.
regen-test-fixtures:
	@mkdir -p "$(ZK_PARAMS)"
	@for f in $(TEST_FIXTURES); do \
		dir="$(TEST_FIXTURE_DIR)/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		$(COMPACTC) --skip-zk --analyzed-ir "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiler/analyzed-ir.sexp"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: test fixtures regenerated"

# Rebuild the runtime tarball the driver installs, from the fork at
# COMPACT_REV (needs Nix and Node). The runtime that this compactc targets is
# not published to npm, so the driver runs the one the fork builds. The
# package's own build scripts need the compiler toolchain, which `npm pack`
# cannot run here, so the packed copy drops them.
vendor-compact-runtime:
	@out="$$(nix --extra-experimental-features 'nix-command flakes' build --no-link \
		--print-out-paths 'github:RomarQ/compact/$(COMPACT_REV)#runtime.forPublish')" || exit 1; \
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
# pair. compactc writes its own --runtime-version into every index.js, and the
# runtime refuses a mismatched minor. After a change of COMPACT_REV, run
# `vendor-compact-runtime` first. When the runtime version moves, run
# `npm install` in tests/conformance. Then run `regen-conformance-fixtures`,
# fix any API drift in the driver, and run this target.
conformance-regen:
	@if [ ! -f "$(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)" ]; then \
		echo "no runtime tarball at $(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)."; \
		echo "It is generated, not committed. Run 'make vendor-compact-runtime' (needs Nix and Node)."; \
		exit 1; \
	fi; \
	for f in $(CONFORMANCE_FIXTURES); do \
		if [ ! -f "$(CONFORMANCE_DIR)/fixtures/$$f/contract/index.js" ]; then \
			echo "no codegen for fixture '$$f'. It is generated, not committed."; \
			echo "Run 'make regen-conformance-fixtures' (needs Docker)."; \
			exit 1; \
		fi; \
	done
	cd $(CONFORMANCE_DIR) && npm ci && node ts-driver/driver.mjs

# Recompile the conformance corpus with the compactc at COMPACT_REV,
# refreshing both compiler outputs each fixture carries. Run
# `conformance-regen` afterwards: new codegen means new goldens.
regen-conformance-fixtures:
	@mkdir -p "$(ZK_PARAMS)"
	@for f in $(CONFORMANCE_FIXTURES); do \
		dir="$(CONFORMANCE_DIR)/fixtures/$$f"; \
		src="$$dir/$$f.compact"; \
		if [ ! -f "$$src" ]; then \
			echo "missing source $$src"; exit 1; \
		fi; \
		echo "Regenerating $$f ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		$(COMPACTC) --skip-zk --analyzed-ir "$$src" "$$dir/compiled.tmp" >/dev/null || exit 1; \
		mkdir -p "$$dir/compiler" "$$dir/contract"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiler/analyzed-ir.sexp"; \
		mv "$$dir/compiled.tmp/contract/index.js" "$$dir/contract/index.js"; \
		rm -rf "$$dir/compiled.tmp"; \
	done; \
	echo "OK: conformance fixtures regenerated (now run 'make conformance-regen')"
