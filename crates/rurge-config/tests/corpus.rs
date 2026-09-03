use rurge_config::config::{LoadOptions, load};
use rurge_config::diagnostic::Diagnostic;
use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .canonicalize()
        .expect("tests/corpus exists")
}

fn conf_files(sub: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(corpus_dir().join(sub))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .conf files in {sub}");
    files
}

/// Render a diagnostic with the file path relative to the corpus directory so snapshots are portable.
fn render(d: &Diagnostic, root: &Path) -> String {
    let location = d
        .span
        .as_ref()
        .map(|s| {
            let rel = s
                .file
                .strip_prefix(root)
                .unwrap_or(&s.file)
                .to_string_lossy()
                .replace('\\', "/");
            format!(" {rel}:{}", s.line)
        })
        .unwrap_or_default();
    format!("{}[{}]{location}: {}", d.severity, d.code, d.message)
}

#[test]
fn valid_corpus_loads_without_errors_and_matches_snapshots() {
    let root = corpus_dir();
    for file in conf_files("valid") {
        let loaded = load(&file, &LoadOptions::for_tests()).unwrap();
        let diags: Vec<String> = loaded
            .diagnostics
            .clone()
            .sorted()
            .iter()
            .map(|d| render(d, &root))
            .collect();
        assert!(
            !loaded.diagnostics.has_errors(),
            "{}: {diags:#?}",
            file.display()
        );
        let name = file.file_stem().unwrap().to_string_lossy().to_string();
        insta::assert_yaml_snapshot!(format!("corpus__{name}"), (loaded.config.summary(), diags));
    }
}

#[test]
fn invalid_corpus_reports_expected_codes() {
    for file in conf_files("invalid") {
        let expect = fs::read_to_string(file.with_extension("expect"))
            .unwrap_or_else(|_| panic!("{}: missing .expect", file.display()));
        let loaded = load(&file, &LoadOptions::for_tests()).unwrap();
        assert!(
            loaded.diagnostics.has_errors(),
            "{} should have errors",
            file.display()
        );
        let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        for code in expect.lines().map(str::trim).filter(|l| !l.is_empty()) {
            assert!(
                codes.contains(&code),
                "{}: expected {code}, got {codes:?}",
                file.display()
            );
        }
    }
}
