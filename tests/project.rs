//! Repository consistency: documentation links and release metadata.

use std::fs;
use std::path::{Path, PathBuf};

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown_documents() -> Vec<PathBuf> {
    let mut documents = vec![project().join("README.md"), project().join("README_ja.md")];
    let mut docs: Vec<PathBuf> = fs::read_dir(project().join("docs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    docs.sort();
    documents.extend(docs);
    documents
}

#[test]
fn every_english_document_has_a_japanese_peer() {
    for document in markdown_documents() {
        let stem = document.file_stem().unwrap().to_str().unwrap();
        if stem.ends_with("_ja") {
            continue;
        }
        let peer = document.with_file_name(format!("{stem}_ja.md"));
        assert!(
            peer.is_file(),
            "{} has no Japanese peer",
            document.display()
        );
    }
}

#[test]
fn local_markdown_links_resolve() {
    let link = regex::Regex::new(r"(!?)\[[^\]]*\]\(([^)]+)\)").unwrap();
    for document in markdown_documents() {
        let text = fs::read_to_string(&document).unwrap();
        for captures in link.captures_iter(&text) {
            let target = captures[2]
                .split_whitespace()
                .next()
                .unwrap()
                .trim_matches(['<', '>']);
            if ["#", "https://", "http://", "mailto:"]
                .iter()
                .any(|prefix| target.starts_with(prefix))
            {
                continue;
            }
            let file = target.split('#').next().unwrap();
            let linked = document.parent().unwrap().join(file);
            assert!(
                linked.exists(),
                "{} links to missing {}",
                document.display(),
                target
            );
        }
    }
}

fn read(path: &str) -> String {
    fs::read_to_string(project().join(path)).unwrap()
}

#[test]
fn release_metadata_follows_the_cargo_version() {
    let manifest: serde_json::Value =
        serde_json::from_str(&read(".release-please-manifest.json")).unwrap();
    assert_eq!(manifest["."], env!("CARGO_PKG_VERSION"));
    let config: serde_json::Value =
        serde_json::from_str(&read("release-please-config.json")).unwrap();
    assert_eq!(config["release-type"], "rust");
    assert_eq!(config["include-component-in-tag"], false);
    assert_eq!(config["include-v-in-tag"], true);
    // The wheel takes its version from Cargo.toml.
    let pyproject = read("pyproject.toml");
    assert!(pyproject.contains("dynamic = [\"version\"]"));
    assert!(pyproject.contains("bindings = \"bin\""));
    assert!(!pyproject.lines().any(|line| line.starts_with("version")));
}

#[test]
fn the_minimum_rust_version_is_tested() {
    let cargo = read("Cargo.toml");
    let declared = cargo
        .lines()
        .find_map(|line| line.strip_prefix("rust-version = "))
        .unwrap()
        .trim_matches('"');
    let workflow = read(".github/workflows/test.yml");
    assert!(
        workflow.contains(&format!("cargo +{declared} check")),
        "test.yml does not check Rust {declared}"
    );
    assert!(Path::new(&project().join("Cargo.lock")).is_file());
}
