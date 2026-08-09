use std::{fs, path::Path};

fn rust_sources(directory: &Path, sources: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            rust_sources(&path, sources);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            sources.push(path);
        }
    }
}

#[test]
fn application_has_no_direct_network_or_event_collection_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    for dependency in [
        "reqwest",
        "hyper",
        "ureq",
        "sentry",
        "opentelemetry",
        "posthog",
        "segment",
    ] {
        assert!(
            !manifest.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with(&format!("{dependency} ="))
                    || line.starts_with(&format!("{dependency}."))
            }),
            "forbidden direct dependency: {dependency}"
        );
    }

    let mut sources = Vec::new();
    rust_sources(&root.join("src"), &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path).expect("Rust source");
        for capability in [
            ".with_http_client(",
            "std::net::",
            "TcpStream",
            "UdpSocket",
            "WebSocket",
        ] {
            assert!(
                !source.contains(capability),
                "{} installs outbound capability {capability}",
                path.display()
            );
        }
    }
}
