//! Preserve the public case facade and serialized corpus contract across decomposition.

use std::path::Path;

use flexpart_gpu::validation::case::ValidationCaseManifest;
use sha2::{Digest, Sha256};

#[test]
fn test_validation_case_serialized_corpus_matches_pre_decomposition_bytes() {
    let case_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus/cases");
    let mut paths: Vec<_> = std::fs::read_dir(case_dir)
        .expect("checked-in case directory")
        .map(|entry| entry.expect("case entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let output = tempfile::tempdir().expect("output directory");
    let mut snapshot = String::new();
    for path in paths {
        let case = ValidationCaseManifest::load_from_file(&path).expect("canonical case");
        let compact = serde_json::to_string(&case).expect("compact serialization");
        let pretty = serde_json::to_string_pretty(&case).expect("pretty serialization");
        let output_path = output.path().join("case.json");
        case.write_to_file(&output_path)
            .expect("validated file serialization");
        assert_eq!(
            std::fs::read(&output_path).expect("serialized file"),
            pretty.as_bytes()
        );
        snapshot.push_str(&format!(
            "{} {:x} {:x}\n",
            case.case_id,
            Sha256::digest(compact.as_bytes()),
            Sha256::digest(pretty.as_bytes()),
        ));
    }
    assert_eq!(
        snapshot,
        include_str!("fixtures/validation-case-serialization-v2.txt").replace("\r\n", "\n")
    );
}
