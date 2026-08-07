//! The conformance gate: replay every case in `cases/` through the Rust IR
//! interpreter and diff the canonical report against the golden emitted by
//! the canonical TS runtime (`expected/`, regenerated with
//! `make conformance-regen`).

use std::fs;
use std::path::{Path, PathBuf};

use conformance::report::{state_report_json, step_report};
use conformance::runner::{Fixture, ScriptedWitnesses, run_step, state_from_value};
use conformance::state_json::state_value_from_json;
use serde_json::Value as Json;

fn base_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

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
    let base = base_dir();
    let case: Json =
        serde_json::from_str(&fs::read_to_string(case_path).expect("case file readable"))
            .expect("case file is JSON");

    let expected_path = base
        .join("expected")
        .join(fixture_name)
        .join(format!("{case_name}.json"));
    let expected: Json =
        serde_json::from_str(&fs::read_to_string(&expected_path).unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run `make conformance-regen`",
                expected_path.display()
            )
        }))
        .expect("golden is JSON");

    let info_path = base
        .join("fixtures")
        .join(fixture_name)
        .join("compiler/contract-info.json");
    let fixture = Fixture::load(&fs::read_to_string(&info_path).expect("contract-info readable"))
        .expect("contract-info parses");

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

    let mut state = state_from_value(initial_sv);
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

        let (args, result) = run_step(&fixture, circuit, state, &args_tagged, &witnesses)
            .unwrap_or_else(|e| panic!("{fixture_name}/{case_name} step {i} ({circuit}): {e}"));
        witnesses
            .assert_drained()
            .unwrap_or_else(|e| panic!("{fixture_name}/{case_name} step {i} ({circuit}): {e}"));

        let arg_refs: Vec<(&str, midnight_contract::runtime::Value)> =
            args.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
        let meta = fixture.circuit_defs(circuit).expect("circuit defs load");
        let type_refs: Vec<(&str, compact_codegen::ir::TypeRef)> = meta
            .arg_types
            .iter()
            .map(|(n, t)| (n.as_str(), t.clone()))
            .collect();
        let rust_report = step_report(circuit, &arg_refs, &type_refs, &meta.structs, &result);
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
    let cases_dir = base_dir().join("cases");
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
