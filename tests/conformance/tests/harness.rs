//! The conformance gate: replay every case in `cases/` through the Rust IR
//! interpreter and diff the canonical report against the golden emitted by
//! the canonical TS runtime (`expected/`, regenerated with
//! `make conformance-regen`).

use std::fs;
use std::path::Path;

use conformance::corpus::{corpus_dir, golden, read_json};
use conformance::report::state_report_json;
use conformance::runner::{
    Fixture, ScriptedWitnesses, balance_from_json, coin_public_key_from_json, run_step,
    state_from_value,
};
use conformance::state_json::state_value_from_json;
use midnight_base_crypto::time::Timestamp;
use serde_json::Value as Json;

/// First differing JSON path between two values, for readable failures.
fn first_diff(path: String, a: &Json, b: &Json, out: &mut Vec<String>) {
    match (a, b) {
        (Json::Object(ma), Json::Object(mb)) => {
            for key in ma.keys().chain(mb.keys().filter(|k| !ma.contains_key(*k))) {
                match (ma.get(key), mb.get(key)) {
                    (Some(va), Some(vb)) => {
                        first_diff(format!("{path}.{key}"), va, vb, out);
                    }
                    (Some(_), None) => out.push(format!("{path}.{key}: missing on the TS side")),
                    (None, Some(_)) => out.push(format!("{path}.{key}: missing on the Rust side")),
                    (None, None) => unreachable!(),
                }
            }
        }
        (Json::Array(va), Json::Array(vb)) => {
            if va.len() != vb.len() {
                out.push(format!(
                    "{path}: array length {} (Rust) vs {} (TS)",
                    va.len(),
                    vb.len()
                ));
            }
            for (i, (ia, ib)) in va.iter().zip(vb).enumerate() {
                first_diff(format!("{path}[{i}]"), ia, ib, out);
            }
        }
        _ if a != b => out.push(format!("{path}: {a} (Rust) vs {b} (TS)")),
        _ => {}
    }
}

fn assert_json_eq(context: &str, rust: &Json, ts: &Json) {
    if rust == ts {
        return;
    }
    let mut diffs = Vec::new();
    first_diff(String::new(), rust, ts, &mut diffs);
    diffs.truncate(20);
    panic!(
        "{context}: Rust interpreter diverges from the canonical TS runtime\n\
         first differing paths:\n  {}\n",
        diffs.join("\n  ")
    );
}

fn run_case_file(case_path: &Path, fixture_name: &str, case_name: &str) {
    let case = read_json(case_path);
    let expected = golden(fixture_name, case_name);
    let fixture = Fixture::open(fixture_name).expect("fixture loads");

    // Seed from the canonical constructor output, and cross-check that the
    // decoder plus the Rust serializer reproduce the TS serialized state
    // byte-for-byte (this also pins ContractMaintenanceAuthority defaults
    // across the two stacks).
    let ctor_state = &expected["constructor"]["state"];
    let initial_sv =
        state_value_from_json(&ctor_state["data"]).expect("golden initial state decodes");
    let rust_ctor_report = state_report_json(&initial_sv);
    assert_json_eq(
        &format!("{fixture_name}/{case_name} constructor state"),
        &rust_ctor_report,
        ctor_state,
    );

    // A case that names no block time, balance or coin public key runs at time
    // 0, with an empty balance and the zero key, as the TS driver does.
    let block_time = Timestamp::from_secs(case.get("blockTime").map_or(0, |t| {
        t.as_u64().expect("blockTime is a whole number of seconds")
    }));
    let balance = balance_from_json(case.get("balance")).expect("case balance parses");
    let coin_public_key =
        coin_public_key_from_json(case.get("coinPublicKey")).expect("case coinPublicKey parses");
    let mut state = state_from_value(initial_sv, balance);
    let steps = case["steps"].as_array().expect("case has steps");
    let expected_steps = expected["steps"].as_array().expect("golden has steps");
    assert_eq!(
        steps.len(),
        expected_steps.len(),
        "step count mismatch between case and golden; run `make conformance-regen`"
    );

    for (i, (step, expected_step)) in steps.iter().zip(expected_steps).enumerate() {
        let circuit = step["circuit"].as_str().expect("step has circuit");
        let args_tagged: Vec<Json> = step["args"].as_array().cloned().unwrap_or_default();
        let witnesses =
            ScriptedWitnesses::from_json(step.get("witnesses")).expect("witness script parses");

        let (rust_report, result) = run_step(
            &fixture,
            circuit,
            state,
            &args_tagged,
            &witnesses,
            block_time,
            Some(coin_public_key),
        )
        .unwrap_or_else(|e| panic!("{fixture_name}/{case_name} step {i} ({circuit}): {e}"));
        witnesses
            .assert_drained()
            .unwrap_or_else(|e| panic!("{fixture_name}/{case_name} step {i} ({circuit}): {e}"));

        assert_json_eq(
            &format!("{fixture_name}/{case_name} step {i} ({circuit})"),
            &rust_report,
            expected_step,
        );

        state = result.state;
    }
}

#[test]
fn conformance_corpus() {
    let cases_dir = corpus_dir().join("cases");
    let mut ran = 0;
    let mut fixtures: Vec<_> = fs::read_dir(&cases_dir)
        .expect("cases dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.is_dir())
        .collect();
    fixtures.sort();
    for fixture_dir in fixtures {
        let fixture_name = fixture_dir
            .file_name()
            .expect("fixture dir name")
            .to_string_lossy()
            .into_owned();
        let mut cases: Vec<_> = fs::read_dir(&fixture_dir)
            .expect("fixture cases dir")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        cases.sort();
        for case_path in cases {
            let case_name = case_path
                .file_stem()
                .expect("case file stem")
                .to_string_lossy()
                .into_owned();
            println!("case {fixture_name}/{case_name}");
            run_case_file(&case_path, &fixture_name, &case_name);
            ran += 1;
        }
    }
    assert!(ran > 0, "no conformance cases found");
}
