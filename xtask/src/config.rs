use std::path::Path;

use craft_xtask::{Cargo, Result, layers, version};

pub fn version(root: &Path) -> version::Config {
    version::Config { cargo: Cargo { root: root.into(), ..Cargo::default() }, update_path_pins: true, ..version::Config::default() }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn prefixed(values: &[&str]) -> Vec<String> {
    values.iter().map(|name| format!("pdfcraft-{name}")).collect()
}

pub fn layers(root: &Path) -> layers::Config {
    let l7 = prefixed(&["platform", "ui-common", "ui-egui", "automation", "testkit", "oracle"]);
    layers::Config {
        root: root.into(),
        layers: [
            ("L0", &["geom", "filters", "crypt", "arlington"][..]),
            ("L1", &["cos"]),
            ("L2", &["content", "model", "color", "fonts", "ops"]),
            ("L3", &["render", "text", "annot", "forms", "js", "xfa", "ml"]),
            (
                "L4",
                &[
                    "edit",
                    "organize",
                    "security",
                    "redact",
                    "audit",
                    "xfdf",
                    "sign",
                    "ocr",
                    "create",
                    "export",
                    "optimize",
                    "preflight",
                    "prepress",
                    "a11y",
                    "compare",
                    "measure",
                    "search",
                    "print",
                    "media",
                    "ai",
                ],
            ),
            ("L5", &["viewport", "tools"]),
            ("L6", &["engine"]),
            ("L7", &["platform", "ui-common", "ui-egui", "automation", "testkit", "oracle"]),
        ]
        .into_iter()
        .map(|(label, members)| (label.into(), prefixed(members)))
        .collect(),
        exempt: names(&["pdfcraft", "pdfcraft-cli", "pdfcraft-web", "xtask"]),
        standalone: prefixed(&["geom", "filters", "crypt", "arlington", "cos"]),
        testkits: prefixed(&["testkit", "oracle"]),
        allowed_edges: [
            ("redact", "edit"),
            ("audit", "edit"),
            ("a11y", "preflight"),
            ("prepress", "preflight"),
            ("export", "ocr"),
            ("sign", "security"),
            ("ui-egui", "ui-common"),
            ("ui-egui", "platform"),
            ("automation", "platform"),
            ("cos", "filters"),
            ("cos", "crypt"),
            ("cos", "geom"),
        ]
        .into_iter()
        .map(|(source, dest)| (format!("pdfcraft-{source}"), format!("pdfcraft-{dest}")))
        .collect(),
        testkits_may_depend_on_testkits: true,
        standalone_may_use_testkits: true,
        external_rules: vec![
            layers::ExternalRule {
                dependencies: names(&["egui", "eframe", "winit", "craft-ui", "egui_extras", "egui_dock", "egui_tiles", "rfd", "wgpu*"]),
                allowed_from: l7.clone(),
                allowed_dev_from: Vec::new(),
            },
            layers::ExternalRule { dependencies: names(&["egui_kittest"]), allowed_from: l7, allowed_dev_from: names(&["*"]) },
        ],
        ..layers::Config::default()
    }
}

pub fn wasm_safe(policy: &layers::Config, name: &str) -> bool {
    !policy.testkits.iter().any(|member| member == name) && policy.layers.iter().any(|(_, members)| members.iter().any(|member| member == name))
}

pub fn describe(policy: &layers::Config, name: &str) -> String {
    if policy.exempt.iter().any(|member| member == name) {
        "exempt".into()
    } else if policy.testkits.iter().any(|member| member == name) {
        "test support".into()
    } else if let Some((label, _)) = policy.layers.iter().find(|(_, members)| members.iter().any(|member| member == name)) {
        if policy.standalone.iter().any(|member| member == name) { format!("{label} standalone") } else { label.clone() }
    } else {
        "UNREGISTERED".into()
    }
}

pub fn format_layer_report(policy: &layers::Config, report: &layers::Report) -> (String, Result) {
    let mut out = format!("Dependency layering (plan/architecture.md §3)\n\n{:<24} {:<14} workspace deps\n", "crate", "layer");
    for krate in &report.crates {
        let deps: Vec<String> = krate
            .deps
            .iter()
            .filter(|dep| dep.workspace)
            .map(|dep| {
                let suffix = match dep.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{suffix}", dep.name.strip_prefix("pdfcraft-").unwrap_or(&dep.name))
            })
            .collect();
        out += &format!("{:<24} {:<14} {}\n", krate.name, describe(policy, &krate.name), deps.join(", "));
    }
    out += "\n";
    let result = if report.ok {
        out += &format!("OK: {} crates, no layering violations.\n", report.crates.len());
        Ok(())
    } else {
        for violation in &report.violations {
            out += &format!("  - {violation}\n");
        }
        Err(format!("{} layering violation(s)", report.violations.len()))
    };
    (out, result)
}

pub fn layer_report(policy: &layers::Config, metadata: &serde_json::Value) -> Result<(String, Result)> {
    let report = layers::check_graph(policy, &layers::from_metadata(metadata)?)?;
    Ok(format_layer_report(policy, &report))
}
