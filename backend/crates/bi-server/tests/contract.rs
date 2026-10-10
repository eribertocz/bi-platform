//! The committed contract `docs/api/openapi.json` is the single source of
//! types for the frontend; it must match what the server implements.

use std::path::Path;

#[test]
fn committed_contract_matches_generated_openapi() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/openapi.json");
    let committed = std::fs::read_to_string(&path).expect("docs/api/openapi.json exists");
    let generated = bi_server::openapi::json();

    let committed_value: serde_json::Value =
        serde_json::from_str(&committed).expect("committed contract is valid JSON");
    let generated_value: serde_json::Value =
        serde_json::from_str(&generated).expect("generated contract is valid JSON");

    assert!(
        committed_value == generated_value,
        "docs/api/openapi.json is out of date with the server.\n\
         Regenerate it from backend/: cargo run -p bi-server -- openapi ../docs/api/openapi.json\n\
         Generated:\n{generated}"
    );
}
