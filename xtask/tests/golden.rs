#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../src/config.rs"]
mod config;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_xtask::{layers, version};
use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pdfcraft-70-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn metadata() -> Value {
    serde_json::from_str(include_str!("golden/metadata.json")).unwrap()
}

fn cli(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask")).current_dir(repo_root()).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output
}

fn child(test: &str, key: &str, root: &Path) -> Vec<u8> {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--quiet", "--test-threads=1"])
        .env(key, root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output.stdout.strip_prefix(b"\nrunning 1 test\n").unwrap().to_vec()
}

#[test]
fn layers_report_matches_original_tree_and_live_dispatch() {
    let (report, result) = config::layer_report(&config::layers(Path::new(".")), &metadata()).unwrap();
    result.unwrap();
    assert_eq!(report.as_bytes(), include_bytes!("golden/layers.stdout"));
    assert_eq!(cli(&["layers"]).stdout, include_bytes!("golden/layers.stdout"));

    let temp = Temp::new();
    let meta_path = temp.0.join("metadata.json");
    std::fs::write(&meta_path, include_bytes!("golden/metadata.json")).unwrap();
    assert_eq!(cli(&["layers", "--metadata", meta_path.to_str().unwrap()]).stdout, include_bytes!("golden/layers.stdout"));
}

#[test]
fn layers_reject_upward_standalone_testkit_and_ui_edges() {
    let policy = config::layers(Path::new("."));
    let graph = layers::from_metadata(&metadata()).unwrap();
    for (source, target, workspace, kind) in [
        ("model", "pdfcraft-engine", true, layers::DepKind::Normal),
        ("forms", "pdfcraft-js", true, layers::DepKind::Normal),
        ("edit", "pdfcraft-redact", true, layers::DepKind::Normal),
        ("cos", "pdfcraft-arlington", true, layers::DepKind::Normal),
        ("filters", "pdfcraft-geom", true, layers::DepKind::Normal),
        ("cos", "pdfcraft-model", true, layers::DepKind::Dev),
        ("cos", "pdfcraft-oracle", true, layers::DepKind::Normal),
        ("cos", "pdfcraft-testkit", true, layers::DepKind::Normal),
        ("render", "pdfcraft-oracle", true, layers::DepKind::Normal),
        ("engine", "egui", false, layers::DepKind::Normal),
        ("engine", "eframe", false, layers::DepKind::Normal),
        ("engine", "winit", false, layers::DepKind::Normal),
        ("engine", "rfd", false, layers::DepKind::Normal),
        ("engine", "wgpu", false, layers::DepKind::Normal),
        ("engine", "wgpu-core", false, layers::DepKind::Normal),
        ("engine", "egui_dock", false, layers::DepKind::Normal),
        ("engine", "craft-ui", false, layers::DepKind::Normal),
        ("engine", "egui_kittest", false, layers::DepKind::Normal),
    ] {
        let mut mutated = graph.clone();
        mutated.iter_mut().find(|krate| krate.name == format!("pdfcraft-{source}")).unwrap().deps.push(layers::Dep {
            name: target.into(),
            kind,
            workspace,
        });
        let report = layers::check_graph(&policy, &mutated).unwrap();
        assert_eq!(report.violations.len(), 1, "{source} -> {target}");
        let (text, res) = config::format_layer_report(&policy, &report);
        assert_eq!(res.unwrap_err(), "1 layering violation(s)");
        assert!(text.contains("  - "), "{text}");
    }
}

#[test]
fn layers_preserve_allowed_sideways_standalone_testkit_ui_and_exempt_rules() {
    let policy = config::layers(Path::new("."));
    let mut graph = layers::from_metadata(&metadata()).unwrap();
    for (source, target, kind, workspace) in
        policy.allowed_edges.iter().map(|(source, target)| (source.as_str(), target.as_str(), layers::DepKind::Normal, true)).chain([
            ("pdfcraft-render", "pdfcraft-oracle", layers::DepKind::Dev, true),
            ("pdfcraft-cos", "pdfcraft-oracle", layers::DepKind::Dev, true),
            ("pdfcraft-geom", "pdfcraft-testkit", layers::DepKind::Dev, true),
            ("pdfcraft-platform", "winit", layers::DepKind::Normal, false),
            ("pdfcraft-ui-egui", "craft-ui", layers::DepKind::Normal, false),
            ("pdfcraft-ui-egui", "egui_kittest", layers::DepKind::Normal, false),
            ("pdfcraft-engine", "egui_kittest", layers::DepKind::Dev, false),
            ("pdfcraft", "egui", layers::DepKind::Normal, false),
            ("pdfcraft-cli", "pdfcraft-ui-egui", layers::DepKind::Normal, true),
            ("xtask", "pdfcraft-ui-egui", layers::DepKind::Normal, true),
        ])
    {
        if let Some(krate) = graph.iter_mut().find(|krate| krate.name == source) {
            krate.deps.push(layers::Dep { name: target.into(), kind, workspace });
        } else {
            graph.push(layers::Crate { name: source.into(), deps: vec![layers::Dep { name: target.into(), kind, workspace }] });
        }
    }
    assert!(layers::check(&policy, &graph).unwrap().is_empty());
    assert_eq!(config::describe(&policy, "pdfcraft-mystery"), "UNREGISTERED");
    graph.push(layers::Crate { name: "pdfcraft-mystery".into(), deps: vec![] });
    assert_eq!(layers::check(&policy, &graph).unwrap().len(), 1);
}

#[test]
fn wasm_selection_preserves_original_layer_boundary() {
    let policy = config::layers(Path::new("."));
    let graph = layers::from_metadata(&metadata()).unwrap();
    let excluded: Vec<_> = graph.iter().filter(|krate| !config::wasm_safe(&policy, &krate.name)).map(|krate| krate.name.as_str()).collect();
    assert_eq!(excluded, ["pdfcraft", "pdfcraft-cli", "pdfcraft-oracle", "pdfcraft-testkit", "pdfcraft-web", "xtask"]);
}

#[test]
fn version_read_matches_original() {
    if let Some(root) = std::env::var_os("PDFCRAFT_GOLDEN_READ_ROOT") {
        version::run(&config::version(Path::new(&root)), &[]).unwrap();
        std::process::exit(0);
    }
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    assert_eq!(child("version_read_matches_original", "PDFCRAFT_GOLDEN_READ_ROOT", &temp.0), include_bytes!("golden/version.stdout"));
    assert_eq!(cli(&["version"]).stdout, include_bytes!("golden/version.stdout"));
}

#[test]
fn version_dry_run_plan_reproduces_manifest_and_path_pins_byte_for_byte() {
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-manifest-before.toml"));
    let policy = config::version(&temp.0);
    let plan = version::compute(&policy, &version::Args::Set("1.2.3-rc.1".into())).unwrap();
    assert_eq!(std::fs::read(temp.0.join("Cargo.toml")).unwrap(), include_bytes!("golden/version-manifest-before.toml"));
    assert!(!temp.0.join("target").exists());
    assert_eq!(plan.before, "0.18.0");
    assert_eq!(plan.after, "1.2.3-rc.1");
    assert_eq!(plan.tag, "v1.2.3-rc.1");
    assert_eq!(version::render(&plan).as_bytes(), include_bytes!("golden/version-set.stdout"));
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.edits[0].2.as_bytes(), include_bytes!("golden/version-manifest-after.toml"));
}

#[test]
fn version_edits_manifest_and_lockfile_identically() {
    if let Some(root) = std::env::var_os("PDFCRAFT_GOLDEN_VERSION_ROOT") {
        version::run(&config::version(Path::new(&root)), &["set".into(), "1.2.3-rc.1".into()]).unwrap();
        std::process::exit(0);
    }
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    temp.write("src/lib.rs", "");
    assert_eq!(
        child("version_edits_manifest_and_lockfile_identically", "PDFCRAFT_GOLDEN_VERSION_ROOT", &temp.0),
        include_bytes!("golden/version-set.stdout")
    );
    assert_eq!(std::fs::read(temp.0.join("Cargo.toml")).unwrap(), include_bytes!("golden/version-after.toml"));
    assert_eq!(std::fs::read(temp.0.join("Cargo.lock")).unwrap(), include_bytes!("golden/version-after.lock"));
}

#[test]
fn version_keeps_parser_crlf_and_rust_version_policy() {
    let policy = config::version(Path::new("."));
    assert!(policy.refresh_lockfile && policy.online_fallback && policy.update_path_pins);
    assert!(!policy.strict_prerelease && !policy.bounded_core && !policy.rollback_on_failure);
    let crlf = "[workspace.package]\r\nversion = \"0.18.0\"\r\n\r\n[workspace.dependencies]\r\nown = { path = \"crates/own\", version = \"0.18.0\" }\r\ndep = { version = \"0.18.0\" }\r\n";
    let expected = "[workspace.package]\r\nversion = \"1.2.3-01\"\r\n\r\n[workspace.dependencies]\r\nown = { path = \"crates/own\", version = \"1.2.3-01\" }\r\ndep = { version = \"0.18.0\" }\r\n";
    assert_eq!(version::rewrite(&policy, crlf, "1.2.3-01").unwrap(), expected);
    for ok in ["0.1.0", "1.20.300", "1.0.0-rc.1", "1.0.0-alpha", "1.0.0-x-y.2"] {
        assert!(version::validate(ok).is_ok(), "{ok}");
    }
    for bad in ["1.0", "1.0.0.0", "v1.0.0", "01.0.0", "1.0.0-", "1.0.0-a..b", "1.0.0+meta", "1.a.0", ""] {
        assert!(version::validate(bad).is_err(), "{bad}");
    }
    let root = repo_root();
    let minor = |manifest: &str| -> u32 {
        let line = manifest.lines().find(|l| l.starts_with("rust-version = \"")).unwrap();
        line.split('"').nth(1).unwrap().split('.').nth(1).unwrap().parse().unwrap()
    };
    let workspace = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let ui = std::fs::read_to_string(root.join("crates/ui-egui/Cargo.toml")).unwrap();
    version::validate(&version::read(&workspace).unwrap()).unwrap();
    assert!(minor(&workspace) >= minor(&ui));
}

#[test]
fn parity_output_and_json_match_original() {
    assert_eq!(cli(&["parity"]).stdout, include_bytes!("golden/parity.stdout"));
    assert_eq!(cli(&["parity", "--json"]).stdout, include_bytes!("golden/parity-json.stdout"));
    let partial = String::from_utf8(cli(&["parity", "--partial"]).stdout).unwrap();
    assert!(partial.contains("partial features and what is missing:"));
}

#[test]
fn shared_hashes_match_original_byte_and_file_digests() {
    let temp = Temp::new();
    let mut bytes = String::new();
    let mut files = String::new();
    for input in [b"".as_slice(), b"abc", b"font bytes", b"png", &[0x61; 1000]] {
        temp.write("input", input);
        bytes += &format!("{}\n", craft_hash::sha256_hex(input));
        files += &format!("{}\n", craft_xtask::sha256_hex(&temp.0.join("input")).unwrap());
    }
    assert_eq!(bytes.as_bytes(), include_bytes!("golden/sha256.stdout"));
    assert_eq!(files.as_bytes(), include_bytes!("golden/sha256.stdout"));
}

#[test]
fn help_text_is_preserved() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask")).current_dir(repo_root()).arg("help").output().unwrap();
    assert!(output.status.success());
    let added = output.stderr.strip_prefix(include_bytes!("golden/help.stderr")).unwrap();
    assert_eq!(added, format!("\nShared task syntax:\n  {}\n  {}\n", version::cli_help(), layers::cli_help()).as_bytes());
}
