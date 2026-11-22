//! Golden tests: every real transaction in `fixtures/` decodes to exactly what
//! `fixtures/decoded/` records. Run with `UPDATE_GOLDEN=1` to rewrite the expected files after
//! a deliberate change, then review the diff.

use std::path::{Path, PathBuf};

use relayscope_core::{TxView, decode_transaction};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn fixture_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("fixtures directory exists")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_fixture_decodes_to_its_golden_file() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let golden_dir = fixtures_dir().join("decoded");
    let files = fixture_files();
    assert!(!files.is_empty(), "no fixtures found");

    let mut mismatches = Vec::new();
    for path in &files {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let decoded = decode_transaction(&TxView::from_rpc_json(&raw).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = serde_json::to_string_pretty(&decoded).unwrap() + "\n";
        let golden = golden_dir.join(path.file_name().unwrap());
        if update {
            std::fs::create_dir_all(&golden_dir).unwrap();
            std::fs::write(&golden, &actual).unwrap();
        } else {
            let expected = std::fs::read_to_string(&golden).unwrap_or_else(|_| {
                panic!("missing {}; run with UPDATE_GOLDEN=1", golden.display())
            });
            if expected != actual {
                mismatches.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "decoded output changed for: {mismatches:?}"
    );
}

#[test]
fn every_golden_file_has_a_fixture() {
    let golden_dir = fixtures_dir().join("decoded");
    let Ok(entries) = std::fs::read_dir(&golden_dir) else {
        return;
    };
    for entry in entries {
        let name = entry.unwrap().file_name();
        assert!(
            fixtures_dir().join(&name).exists(),
            "decoded/{} has no fixture",
            name.to_string_lossy()
        );
    }
}
