#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

#[test]
fn isolated_launcher_copies_files_and_overrides_inherited_application_storage() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("fake-textify");
    fs::write(&binary, "#!/bin/sh\nprintf 'CAPTURE_DATA=%s\\nCAPTURE_DEV=%s\\n' \"$TEXTIFY_DATA_DIR\" \"$TEXTIFY_DEV_INSTANCE\"\nfor path in \"$@\"; do printf 'CAPTURE_FILE=%s\\n' \"$path\"; done\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let original = directory.path().join("original with spaces.json");
    fs::write(&original, "{\"original\": true}\n").unwrap();
    let real_data = directory.path().join("real-data");
    fs::create_dir(&real_data).unwrap();
    fs::write(
        real_data.join("session.json"),
        "real unsaved session sentinel",
    )
    .unwrap();

    let output = Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/run-isolated.sh"))
        .arg("--binary")
        .arg(&binary)
        .arg("--minimap-demo")
        .arg("--")
        .arg(&original)
        .env("TEXTIFY_DATA_DIR", &real_data)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let data = Path::new(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("CAPTURE_DATA="))
            .unwrap(),
    );
    let instance = data.parent().unwrap();
    assert!(
        instance
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("textify-dev.")
    );
    assert_ne!(data, real_data);
    assert!(stdout.contains("CAPTURE_DEV=1"));
    let opened: Vec<_> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("CAPTURE_FILE="))
        .map(Path::new)
        .collect();
    assert_eq!(opened.len(), 5);
    assert!(
        opened
            .iter()
            .all(|path| path.starts_with(instance.join("documents")))
    );
    let json = fs::read_to_string(opened[3]).unwrap();
    assert_eq!(json.lines().count(), 10_002);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1_000
    );
    assert_eq!(
        fs::read_to_string(opened[2]).unwrap().lines().count(),
        50_000
    );
    fs::write(opened[4], "changed test copy").unwrap();
    assert_eq!(
        fs::read_to_string(original).unwrap(),
        "{\"original\": true}\n"
    );
    assert_eq!(
        fs::read_to_string(real_data.join("session.json")).unwrap(),
        "real unsaved session sentinel"
    );
    assert_eq!(fs::read_dir(real_data).unwrap().count(), 1);

    let copy = if cfg!(target_os = "macos") {
        instance.join("Textify Dev.app/Contents/MacOS/Textify Dev")
    } else {
        instance.join("textify-dev")
    };
    assert!(
        !fs::symlink_metadata(&copy)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let copied_bytes = fs::read(&copy).unwrap();
    fs::write(&binary, "a later build").unwrap();
    assert_eq!(fs::read(copy).unwrap(), copied_bytes);
    fs::remove_dir_all(instance).unwrap();
}
