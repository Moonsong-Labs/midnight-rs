# Development tasks for the midnight-rs workspace.
# Run `make` (or `make help`) to list targets. The CI workflow calls these
# same targets, so local and CI stay in sync.

CARGO ?= cargo

# Compiling contracts needs the compactc of the fork RomarQ/compact, which adds
# the --analyzed-ir flag that writes the analyzed-ir.sexp artifact the SDK
# consumes. COMPACT_REV is the fork commit this repository tests, and a
# workflow on the fork publishes its image. Override COMPACTC to use your own
# build of the fork.
#
# After a change of COMPACT_REV:
# 1. Run `make vendor-compact-runtime`. Then run
#    `npm install ./ts-driver/vendor/compact-runtime.tgz` in tests/conformance,
#    because a plain `npm install` keeps the lockfile entry of the old tarball.
# 2. Run `make goldens`, and fix any API drift in
#    tests/conformance/ts-driver/driver.mjs. The fixtures, the contracts and the
#    goldens carry COMPACT_REV in their fingerprints, so each target that needs
#    them builds them again.
# 3. Run `make check-natives`, and update COMPACT_NATIVES with what it reports.
# 4. For a new compiler `major.minor`, change SUPPORTED_COMPILER_FAMILY in
#    crates/compact/codegen/src/types.rs and the version in
#    crates/compact/bindgen-macro/tests/ui/fail/version-mismatch.stderr.
# 5. Replace the old commit in README.md, docs/compact-natives.md and
#    crates/midnight-contract/tests/fixtures/README.md.
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
PROOF_SERVER_URL := http://127.0.0.1:6300
DEV_SEED       := 0000000000000000000000000000000000000000000000000000000000000001

# Examples that run against the devnet with no extra env.
# wallet-sync gets its devnet env from a dedicated target.
EXAMPLES  := counter private-state contract-maintenance shielded-transfer combine-and-sponsor shielded-swap
CONTRACTS := call-context counter counter-zkir-v3 events receive-shielded secret-counter shielded-mint unshielded-payout
# The contracts that compile for ZKIR v3. Only a ledger 9 chain verifies their keys.
ZKIR_V3_CONTRACTS := counter-zkir-v3
# The compiler flag of contract $$c, which only a ZKIR v3 contract has. The
# pattern opens with `(` so that bash 3.2, the macOS /bin/sh, parses it inside $(...).
CONTRACT_ZKIR = case " $(ZKIR_V3_CONTRACTS) " in (*" $$c "*) echo --feature-zkir-v3 ;; esac
# The fingerprint of contract $$c: its source and its flag under the compiler at
# COMPACT_REV. POSIX cksum, so a fingerprint from the Linux CI job matches on
# macOS.
CONTRACT_CKSUM = { echo $(COMPACT_REV); $(CONTRACT_ZKIR); cat devnet/contracts/$$c/*.compact; } | cksum

# Interpreter test fixtures: crates/midnight-contract/tests/fixtures/<name>/<name>.compact.
TEST_FIXTURES := bboard counter election tiny
TEST_FIXTURE_DIR := crates/midnight-contract/tests/fixtures
# Codegen test fixtures: tests/fixtures/sources/<name>.compact compiles to
# tests/fixtures/compiled/<name>/.
CODEGEN_FIXTURES := cross-contract gateway many-fields mint-probe pairs results struct-shapes tuple-shapes zerocash
# Parser test fixtures: crates/compact/analyzed-ir/tests/fixtures/<name>/<name>.compact.
# The SDK refuses each of these contracts at load, so the codegen tests cannot read them.
PARSER_FIXTURES := adt-defaults
PARSER_FIXTURE_DIR := crates/compact/analyzed-ir/tests/fixtures

# Conformance corpus: tests/conformance/fixtures/<name>/<name>.compact. The two
# executors read different compiler outputs of a fixture: the Rust IR
# interpreter reads `compiler/analyzed-ir.sexp`, and the ts-driver runs
# `contract/index.js` against the canonical @midnight-ntwrk/compact-runtime.
CONFORMANCE_FIXTURES := bboard containers counter curve25519 defaults events indexing keccak kernel \
                        loops ops peers scopes secp256k1 secp256r1 sha512 shadowing slices structs \
                        tiny trees vectors
# The fixtures that compile for ZKIR v3. Without the flag, the compiler declares
# no sha512 and no foreign-curve types. Its ZKIR v2 backend also rejects
# keccak256 in a circuit that needs a proof, and the driver runs only such
# circuits.
CONFORMANCE_ZKIR_V3_FIXTURES := curve25519 keccak secp256k1 secp256r1 sha512
CONFORMANCE_DIR := tests/conformance
# The runtime tarball the driver installs, which `vendor-compact-runtime` builds
# from COMPACT_REV, and the file that names the revision it came from. The name
# carries no version, so package.json never moves with the runtime.
COMPACT_RUNTIME_TGZ := ts-driver/vendor/compact-runtime.tgz
COMPACT_RUNTIME_REV := ts-driver/vendor/compact-runtime.rev
# The fork sources that the runtime build reads: export-version.ss imports the
# compiler's (field) library.
COMPACT_RUNTIME_SRC := runtime compiler/json.ss compiler/utils.ss compiler/field.ss third_party/compiler/state-case.ss
# The source file of the interpreter's native check, which holds COMPACT_NATIVES.
NATIVES_TEST := crates/compact/interpreter/src/lib.rs

# Each fixture as <output directory>:<source>. `make fixtures` writes the
# analyzed IR to <output directory>/compiler/, and the TS codegen of a
# conformance fixture to <output directory>/contract/.
FIXTURES := $(foreach f,$(CONFORMANCE_FIXTURES),$(CONFORMANCE_DIR)/fixtures/$(f):$(CONFORMANCE_DIR)/fixtures/$(f)/$(f).compact) \
            $(foreach f,$(TEST_FIXTURES),$(TEST_FIXTURE_DIR)/$(f):$(TEST_FIXTURE_DIR)/$(f)/$(f).compact) \
            $(foreach f,$(CODEGEN_FIXTURES),tests/fixtures/compiled/$(f):tests/fixtures/sources/$(f).compact) \
            $(foreach f,$(PARSER_FIXTURES),$(PARSER_FIXTURE_DIR)/$(f):$(PARSER_FIXTURE_DIR)/$(f)/$(f).compact)
# The compiler flag of the fixture in $$dir, which only a ZKIR v3 conformance
# fixture has. The pattern opens with `(` for bash 3.2, as CONTRACT_ZKIR does.
FIXTURE_ZKIR = case " $(foreach f,$(CONFORMANCE_ZKIR_V3_FIXTURES),$(CONFORMANCE_DIR)/fixtures/$(f)) " in (*" $$dir "*) echo --feature-zkir-v3 ;; esac
# The fingerprint of the fixture $$src, as CONTRACT_CKSUM is for a contract.
FIXTURE_CKSUM = { echo $(COMPACT_REV); $(FIXTURE_ZKIR); cat "$$src"; } | cksum
# The fingerprint of the goldens: what the driver reads, under the compiler and
# the runtime at COMPACT_REV. Make sorts the file list, so it is the same on
# Linux and macOS.
GOLDENS_CKSUM = { echo $(COMPACT_REV) $(CONFORMANCE_ZKIR_V3_FIXTURES); \
	cat $(sort $(wildcard $(CONFORMANCE_DIR)/fixtures/*/*.compact $(CONFORMANCE_DIR)/cases/*/*.json)) \
		$(CONFORMANCE_DIR)/ts-driver/driver.mjs $(CONFORMANCE_DIR)/package-lock.json; } | cksum

.PHONY: help fmt fmt-check clippy doc check test build audit ci \
        dev-up dev-wait proof-server-wait dev-settle dev-down dev-status dev-logs \
        test-e2e test-e2e-node-restart examples e2e run-wallet-sync \
        fork-up fork-upgrade fork-test fork-down \
        contracts compile-contracts fixtures goldens conformance \
        vendor-compact-runtime check-natives

help:
	@echo "midnight-rs make targets:"
	@echo ""
	@echo "  Lint / build / test (a missing or stale contract or fixture compiles first, in Docker;"
	@echo "  test also writes the goldens, which needs Node 22)"
	@echo "    fmt           cargo fmt --all"
	@echo "    fmt-check     cargo fmt --all --check"
	@echo "    clippy        cargo clippy --workspace --all-targets -- -D warnings"
	@echo "    doc           cargo doc --workspace --no-deps (RUSTDOCFLAGS=-D warnings)"
	@echo "    check         cargo check --workspace"
	@echo "    test          cargo test --workspace"
	@echo "    build         cargo build --workspace"
	@echo "    audit         cargo audit (fails on vulnerabilities; warnings allowed)"
	@echo "    ci            fmt-check + clippy + doc + test + audit (the CI gates)"
	@echo ""
	@echo "  Devnet (node, indexer and proof server via $(DEVNET_COMPOSE); DEVNET_LEDGER=9 for ledger 9)"
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
	@echo "    contracts           compile each devnet contract whose compiled/ output is missing or stale (needs Docker)"
	@echo "    compile-contracts   recompile devnet/contracts/*, with keys (needs Docker)"
	@echo "    fixtures            compile each test and conformance fixture whose output is missing or stale (needs Docker)"
	@echo "    goldens             write the conformance goldens with the TS driver when stale (needs Node)"
	@echo "    conformance         run the interpreter-vs-TS-runtime conformance gate"
	@echo "    vendor-compact-runtime  build the driver's compact-runtime at COMPACT_REV (needs curl, Docker and Node)"
	@echo "    check-natives       check the interpreter's native list against the compiler at COMPACT_REV (needs curl)"

# ============================================================
# Lint / build / test  (mirrors .github/workflows/ci.yml)
# ============================================================

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy: contracts fixtures
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Rustdoc's own lints: a link to an item that moved or was renamed, a public
# doc pointing at a private item, a bare URL. `cargo check` and `cargo clippy`
# see none of them, so without this gate they only surface for whoever next
# builds the docs.
doc: contracts fixtures
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps

check: contracts fixtures
	$(CARGO) check --workspace

test: contracts fixtures goldens
	$(CARGO) test --workspace

build: contracts fixtures
	$(CARGO) build --workspace

# cargo audit checks the lockfile against the RustSec advisory database.
# Exit code 0 means no vulnerabilities; the "unmaintained" warnings on
# transitive deps we don't control (paste, bincode, libsecp256k1,
# number_prefix) are allowed and do not fail the gate.
audit:
	$(CARGO) audit

ci: fmt-check clippy doc test audit
	@echo "OK: local CI gates passed"

# ============================================================
# Devnet (node, indexer and proof server)
# ============================================================

dev-up:
	docker compose -f $(DEVNET_COMPOSE) up -d
	@$(MAKE) --no-print-directory dev-wait
	@$(MAKE) --no-print-directory proof-server-wait

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

# Not part of dev-wait: the fork devnet reuses dev-wait and has no proof server.
proof-server-wait:
	@echo "Waiting for the proof server..."
	@for _ in $$(seq 1 60); do \
		curl -sf $(PROOF_SERVER_URL)/health >/dev/null 2>&1 && { echo "Proof server ready."; exit 0; }; \
		sleep 2; \
	done; \
	echo "ERROR: the proof server did not answer $(PROOF_SERVER_URL)/health"; \
	docker compose -f $(DEVNET_COMPOSE) logs proof-server; \
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
fork-test: contracts
	$(E2E_ENV) MIDNIGHT_FORK_UPGRADE_CMD="$(MAKE) -C $(CURDIR) --no-print-directory fork-upgrade" \
		$(CARGO) test -p midnight-contract --test fork_crossing -- --show-output

fork-down:
	docker compose -f $(FORK_COMPOSE) down

# ============================================================
# Against a running devnet
# ============================================================

E2E_ENV := MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_E2E=1 \
	MIDNIGHT_LEDGER=$(DEVNET_LEDGER) MIDNIGHT_PROOF_SERVER_URL=$(PROOF_SERVER_URL)

# The devnet integration tests.
test-e2e: contracts fixtures
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test node_e2e -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-wallet --test integration -- --show-output --test-threads=1
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test balance_bare_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test prove_once_per_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_registration_offer -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_registration_submit -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_registration_target -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test dust_deregistration_submit -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test transaction_hash_identity -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test recover_unencrypted_mint -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test proving_outside_the_wallet_lock -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test unshielded_payout_to_user -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test call_context -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test simulate_call -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test build_only_private_state -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test emit_event -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test zkir_v3_local_proving -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test remote_proving -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-indexer-client --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-provider --test devnet -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test mint_external_recipient -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test receive_shielded_coin -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test deploy_shielded_offer -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test failed_submit_hands_back -- --show-output
	$(E2E_ENV) $(CARGO) test -p midnight-contract --test e2e_contracts -- --show-output --test-threads=1

# Restarts the node container, so it disrupts every other test talking to it.
# Kept out of test-e2e; run it last, on its own.
test-e2e-node-restart:
	$(E2E_ENV) MIDNIGHT_NODE_CONTAINER=$(NODE_CONTAINER) \
		$(CARGO) test -p midnight-provider --test devnet -- --ignored --show-output

# wallet-sync needs devnet env; this explicit target sets it (and overrides the
# run-% pattern below).
run-wallet-sync:
	MIDNIGHT_NODE_URL=$(NODE_WS) MIDNIGHT_INDEXER_URL=$(INDEXER_URL) MIDNIGHT_NETWORK=undeployed \
		MIDNIGHT_WALLET_SEED=$(DEV_SEED) $(CARGO) run -p example-wallet-sync

# Run any other example: `make run-counter`, `make run-private-state`, ...
run-%: contracts
	$(CARGO) run -p example-$*

examples: contracts
	@for ex in $(EXAMPLES); do \
		echo "=== example-$$ex ==="; \
		$(MAKE) --no-print-directory dev-settle || exit 1; \
		$(CARGO) run -p example-$$ex || exit 1; \
	done

e2e: contracts dev-up
	@$(MAKE) --no-print-directory examples
	@$(MAKE) --no-print-directory dev-down

# ============================================================
# Contracts (Compact, compiled in the compiler image)
# ============================================================

# Compile each devnet contract whose output is missing, or whose fingerprint
# differs from the one its output carries. A checksum, not an mtime: in CI the
# output comes from another job, so its mtime says nothing about its source.
# The analyzed IR is checked too: a checkout of a commit that tracks compiled/
# can leave the untracked fingerprint behind with no output.
contracts:
	@for c in $(CONTRACTS); do \
		out="devnet/contracts/$$c/compiled"; \
		[ -f "$$out/compiler/analyzed-ir.sexp" ] \
			&& [ "$$($(CONTRACT_CKSUM))" = "$$(cat "$$out/source.cksum" 2>/dev/null)" ] \
			|| $(MAKE) --no-print-directory compile-contracts CONTRACTS=$$c || exit 1; \
	done

# Recompile each contract into its compiled/ directory, in the layout the
# compiler writes: compiler/analyzed-ir.sexp, keys/ and zkir/. The contract!
# macro reads the artifact, and with_zk_config takes compiled/. The SDK reads
# none of the compiler's other output, such as the TS contract/ directory, so
# the target drops it. The fingerprint is written last, so it exists only
# beside a whole output.
compile-contracts:
	@mkdir -p "$(ZK_PARAMS)"
	@set -e; \
	for c in $(CONTRACTS); do \
		dir="devnet/contracts/$$c"; \
		echo "Compiling $$dir ..."; \
		rm -rf "$$dir/compiled.tmp"; \
		$(COMPACTC) $$($(CONTRACT_ZKIR)) --analyzed-ir "$$dir"/*.compact "$$dir/compiled.tmp"; \
		rm -rf "$$dir/compiled"; \
		mkdir -p "$$dir/compiled/compiler"; \
		mv "$$dir/compiled.tmp/keys" "$$dir/compiled.tmp/zkir" "$$dir/compiled/"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiled/compiler/"; \
		rm -rf "$$dir/compiled.tmp"; \
		$(CONTRACT_CKSUM) > "$$dir/compiled/source.cksum"; \
	done; \
	echo "OK: contracts compiled"

# Compile each test and conformance fixture whose analyzed IR is missing, or
# whose fingerprint differs from the one its output carries, as `contracts`
# does for the devnet contracts. No fixture needs keys.
fixtures:
	@mkdir -p "$(ZK_PARAMS)"
	@set -e; \
	for pair in $(FIXTURES); do \
		dir="$${pair%%:*}"; \
		src="$${pair#*:}"; \
		[ -f "$$dir/compiler/analyzed-ir.sexp" ] \
			&& [ "$$($(FIXTURE_CKSUM))" = "$$(cat "$$dir/compiler/source.cksum" 2>/dev/null)" ] \
			&& continue; \
		echo "Compiling $$src ..."; \
		rm -rf "$$dir/compiled.tmp" "$$dir/compiler" "$$dir/contract"; \
		$(COMPACTC) --skip-zk $$($(FIXTURE_ZKIR)) --analyzed-ir "$$src" "$$dir/compiled.tmp" >/dev/null; \
		mkdir -p "$$dir/compiler"; \
		mv "$$dir/compiled.tmp/compiler/analyzed-ir.sexp" "$$dir/compiler/"; \
		case "$$dir" in ($(CONFORMANCE_DIR)/fixtures/*) \
			mkdir -p "$$dir/contract"; \
			mv "$$dir/compiled.tmp/contract/index.js" "$$dir/contract/" ;; \
		esac; \
		rm -rf "$$dir/compiled.tmp"; \
		$(FIXTURE_CKSUM) > "$$dir/compiler/source.cksum"; \
	done

# Rebuild the runtime tarball the driver installs, from the runtime sources of
# the fork at COMPACT_REV (needs curl, Docker and Node). The runtime that this
# compactc targets is not published to npm. Its build writes src/version.ts
# with the Chez Scheme of the compactc image, then runs tsc. The packed
# manifest drops the build scripts, because `npm pack` runs them, and sorts its
# keys, the order of the tarball that package-lock.json pins.
vendor-compact-runtime:
	@set -e; \
	work="$(CURDIR)/target/compact-runtime"; \
	rm -rf "$$work"; \
	mkdir -p "$$work/src" "$$work/pack"; \
	curl -fsSL 'https://github.com/RomarQ/compact/archive/$(COMPACT_REV).tar.gz' \
		| tar -xz -C "$$work/src" --strip-components=1 \
			$(addprefix compact-$(COMPACT_REV)/,$(COMPACT_RUNTIME_SRC)); \
	cd "$$work/src/runtime"; \
	npm ci --ignore-scripts --no-audit --no-fund --silent; \
	docker run --rm --user $$(id -u):$$(id -g) -v "$$work/src:/src" -w /src/runtime \
		-e CHEZSCHEMELIBDIRS=../compiler:../third_party/compiler $(COMPACTC_IMAGE) \
		'/nix/store/*-chez-scheme-*/bin/scheme --script export-version.ss'; \
	./node_modules/.bin/tsc -b; \
	cp -R dist package.json README.md "$$work/pack/"; \
	cd "$$work/pack"; \
	node -e 'const fs = require("fs"); const sort = (v) => Array.isArray(v) ? v.map(sort) : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sort(v[k])])) : v; const j = sort(JSON.parse(fs.readFileSync("package.json"))); delete j.scripts; delete j.devDependencies; fs.writeFileSync("package.json", JSON.stringify(j, null, 2) + "\n")'; \
	version="$$(node -p 'require("./package.json").version')"; \
	packed="$$(npm pack --silent --pack-destination "$$work")"; \
	dest="$(CURDIR)/$(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ)"; \
	mkdir -p "$$(dirname "$$dest")"; \
	mv "$$work/$$packed" "$$dest"; \
	rm -rf "$$work"; \
	echo "$(COMPACT_REV)" > "$(CURDIR)/$(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_REV)"; \
	echo "OK: compact-runtime $$version vendored"

# Run the conformance gate: the Rust IR interpreter against the goldens
# emitted by the canonical TS runtime (already part of `make test`; this
# target is the focused loop).
conformance: fixtures goldens
	$(CARGO) test -p conformance

# Write the conformance goldens: the TS driver runs every case through the
# canonical @midnight-ntwrk/compact-runtime (needs Node 22+). The target skips
# when every case has a golden and the goldens carry the fingerprint of what
# the driver reads. A checkout can delete tracked goldens and leave the ignored
# fingerprint behind, hence the first check. It builds the runtime tarball only
# when the tarball is missing or comes from another COMPACT_REV.
# NB: the generated contract/index.js and the vendored runtime are a matched
# pair. compactc writes its own --runtime-version into every index.js, and the
# runtime refuses a mismatched minor. So after a change of COMPACT_REV, follow
# the steps above COMPACT_REV.
goldens: fixtures
	@missing=; \
	for c in $(wildcard $(CONFORMANCE_DIR)/cases/*/*.json); do \
		[ -f "$(CONFORMANCE_DIR)/expected/$${c#$(CONFORMANCE_DIR)/cases/}" ] || missing=1; \
	done; \
	[ -z "$$missing" ] \
		&& [ "$$($(GOLDENS_CKSUM))" = "$$(cat $(CONFORMANCE_DIR)/expected/source.cksum 2>/dev/null)" ] \
		&& exit 0; \
	[ -f $(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_TGZ) ] \
		&& [ "$$(cat $(CONFORMANCE_DIR)/$(COMPACT_RUNTIME_REV) 2>/dev/null)" = "$(COMPACT_REV)" ] \
		|| $(MAKE) --no-print-directory vendor-compact-runtime || exit 1; \
	rm -rf $(CONFORMANCE_DIR)/expected; \
	( cd $(CONFORMANCE_DIR) && npm ci --silent && node ts-driver/driver.mjs ) || exit 1; \
	$(GOLDENS_CKSUM) > $(CONFORMANCE_DIR)/expected/source.cksum; \
	echo "OK: conformance goldens written"

# Check that COMPACT_NATIVES, in the native check of the interpreter's unit
# tests, holds the runtime symbols of the compiler's native tables at
# COMPACT_REV (needs curl). The symbol, not the source name, identifies a
# native: the ZKIR v3 table declares `ecAdd` again for each foreign curve.
check-natives:
	@set -e; \
	base='https://raw.githubusercontent.com/RomarQ/compact/$(COMPACT_REV)/compiler'; \
	tmp="$$(mktemp -d)"; \
	curl -fsSL "$$base/midnight-natives.ss" "$$base/zkir-v3-natives.ss" \
		| grep -o '"__compactRuntime\.[A-Za-z0-9_]*"' \
		| sed 's/^"__compactRuntime\.//; s/"$$//' | LC_ALL=C sort -u > "$$tmp/compiler"; \
	sed -n '/const COMPACT_NATIVES/,/];/p' $(NATIVES_TEST) \
		| grep -o '"[A-Za-z0-9_]*"' | tr -d '"' | LC_ALL=C sort -u > "$$tmp/test"; \
	if [ ! -s "$$tmp/compiler" ] || [ ! -s "$$tmp/test" ]; then \
		echo "no native symbols read from the compiler tables or from $(NATIVES_TEST)"; exit 1; \
	fi; \
	if ! cmp -s "$$tmp/compiler" "$$tmp/test"; then \
		echo "COMPACT_NATIVES in $(NATIVES_TEST) does not match the compiler at COMPACT_REV."; \
		echo "Add:"; LC_ALL=C comm -23 "$$tmp/compiler" "$$tmp/test" | sed 's/^/  /'; \
		echo "Remove:"; LC_ALL=C comm -13 "$$tmp/compiler" "$$tmp/test" | sed 's/^/  /'; \
		rm -rf "$$tmp"; exit 1; \
	fi; \
	rm -rf "$$tmp"; \
	echo "OK: COMPACT_NATIVES matches the compiler at COMPACT_REV"
