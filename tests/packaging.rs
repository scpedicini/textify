const INFO_PLIST: &str = include_str!("../packaging/Info.plist");
const ICON_SVG: &str = include_str!("../packaging/Textify.svg");
const ICON_ICNS: &[u8] = include_bytes!("../packaging/Textify.icns");
const LOCAL_INSTALLER: &str = include_str!("../scripts/install-local.sh");
const MACOS_PACKAGER: &str = include_str!("../scripts/package-macos-release.sh");
const MACOS_SMOKE_TEST: &str = include_str!("../scripts/smoke-macos.sh");
const LINUX_PACKAGER: &str = include_str!("../scripts/package-linux.sh");
const LINUX_SMOKE_TEST: &str = include_str!("../scripts/smoke-linux.sh");
const LINUX_CONTROL: &str = include_str!("../packaging/linux/control");
const LINUX_DESKTOP: &str = include_str!("../packaging/linux/com.shaun.textify.desktop");
const WINDOWS_PACKAGER: &str = include_str!("../scripts/package-windows.ps1");
const WINDOWS_INSTALLER: &str = include_str!("../packaging/windows/Textify.iss");
const WINDOWS_ICON: &[u8] = include_bytes!("../packaging/windows/Textify.ico");
const RELEASE_WORKFLOW: &str = include_str!("../.github/workflows/prod-release.yml");
const BUILD_WORKFLOW: &str = include_str!("../.github/workflows/build-platforms.yml");
const PROD_PULL_REQUEST_WORKFLOW: &str = include_str!("../.github/workflows/prod-pr-check.yml");
const VERSION_BUMPER: &str = include_str!("../scripts/bump-version.sh");

#[test]
fn macos_bundle_declares_the_textify_icon_resource() {
    assert!(INFO_PLIST.contains("<key>CFBundleIconFile</key>"));
    assert!(INFO_PLIST.contains("<string>Textify.icns</string>"));
}

#[test]
fn macos_bundle_registers_as_a_text_document_editor() {
    assert!(INFO_PLIST.contains("<key>CFBundleDocumentTypes</key>"));
    assert!(INFO_PLIST.contains("<string>Editor</string>"));
    assert!(INFO_PLIST.contains("<string>Alternate</string>"));
    for content_type in [
        "public.text",
        "public.plain-text",
        "public.source-code",
        "public.json",
        "public.html",
        "public.css",
        "public.shell-script",
    ] {
        assert!(
            INFO_PLIST.contains(&format!("<string>{content_type}</string>")),
            "missing {content_type}"
        );
    }
}

#[test]
fn local_installer_refreshes_launch_services_registration() {
    assert!(LOCAL_INSTALLER.contains("LaunchServices.framework/Support/lsregister"));
    assert!(LOCAL_INSTALLER.contains("-u \"${stale_distribution_bundle}\""));
    assert!(LOCAL_INSTALLER.contains("\"${launch_services}\" -f \"${bundle_link}\""));
}

#[test]
fn icon_source_stays_editable_and_vector_native() {
    assert!(ICON_SVG.contains("viewBox=\"0 0 1024 1024\""));
    assert!(ICON_SVG.contains("Textify application icon"));
    assert!(!ICON_SVG.contains("data:image"));
    assert!(!ICON_SVG.contains("<text"));
}

#[test]
fn committed_icon_is_a_complete_icns_container() {
    assert!(ICON_ICNS.len() > 8);
    assert_eq!(&ICON_ICNS[..4], b"icns");
    let declared_size = u32::from_be_bytes(ICON_ICNS[4..8].try_into().expect("ICNS size"));
    assert_eq!(declared_size as usize, ICON_ICNS.len());
}

#[test]
fn macos_distribution_is_a_copied_universal_bundle() {
    assert!(MACOS_PACKAGER.contains("aarch64-apple-darwin,x86_64-apple-darwin"));
    assert!(MACOS_PACKAGER.contains("lipo -create"));
    assert!(MACOS_PACKAGER.contains("ditto -c -k"));
    assert!(!MACOS_PACKAGER.contains("ln -s"));
    assert!(MACOS_SMOKE_TEST.contains("sleep 10"));
}

#[test]
fn linux_distribution_has_archive_deb_and_desktop_integration() {
    assert!(LINUX_PACKAGER.contains("cargo build --locked --release --bin textify"));
    assert!(LINUX_PACKAGER.contains("dpkg-deb --root-owner-group --build"));
    assert!(LINUX_CONTROL.contains("fonts-dejavu-core"));
    assert!(LINUX_PACKAGER.contains("tar -C"));
    assert!(LINUX_DESKTOP.contains("Exec=textify %F"));
    assert!(LINUX_DESKTOP.contains("MimeType=text/plain;"));
    assert!(LINUX_SMOKE_TEST.contains("xvfb-run"));
    assert!(LINUX_SMOKE_TEST.contains("timeout 10s"));
}

#[test]
fn windows_distribution_has_embedded_icon_portable_zip_and_installer() {
    assert!(WINDOWS_ICON.starts_with(&[0, 0, 1, 0]));
    assert!(WINDOWS_PACKAGER.contains("cargo build --locked --release --bin textify"));
    assert!(WINDOWS_PACKAGER.contains("Compress-Archive"));
    assert!(WINDOWS_PACKAGER.contains("Inno Setup 6"));
    assert!(WINDOWS_INSTALLER.contains("PrivilegesRequired=lowest"));
    assert!(WINDOWS_INSTALLER.contains("SupportedTypes"));
}

#[test]
fn shared_build_covers_every_native_runner_and_smoke_test() {
    assert!(BUILD_WORKFLOW.contains("workflow_call"));
    assert!(BUILD_WORKFLOW.contains("runs-on: ubuntu-22.04"));
    assert!(BUILD_WORKFLOW.contains("runs-on: windows-2022"));
    assert!(BUILD_WORKFLOW.contains("runs-on: macos-15"));
    assert!(BUILD_WORKFLOW.contains("smoke-macos.sh"));
    assert!(BUILD_WORKFLOW.contains("smoke-linux.sh"));
    // lipo reads the file before the command, so the reversed form always fails.
    assert!(BUILD_WORKFLOW.contains("MacOS/Textify -verify_arch arm64 x86_64"));
    for artifact in [
        "release-linux-x64",
        "release-windows-x64",
        "release-macos-universal",
    ] {
        assert!(BUILD_WORKFLOW.contains(artifact), "missing {artifact}");
    }
}

#[test]
fn prod_workflow_gates_publication_on_all_native_runner_builds() {
    assert!(RELEASE_WORKFLOW.contains("branches:\n      - prod"));
    assert!(RELEASE_WORKFLOW.contains("uses: ./.github/workflows/build-platforms.yml"));
    assert!(RELEASE_WORKFLOW.contains("needs:\n      - version\n      - build"));
    assert!(RELEASE_WORKFLOW.contains("permissions:\n      contents: write"));
    assert!(RELEASE_WORKFLOW.contains("gh release create"));
    assert!(RELEASE_WORKFLOW.contains("SHA256SUMS"));
}

#[test]
fn prod_pull_requests_run_the_same_platform_builds() {
    assert!(PROD_PULL_REQUEST_WORKFLOW.contains("pull_request:"));
    assert!(PROD_PULL_REQUEST_WORKFLOW.contains("branches:\n      - prod"));
    assert!(PROD_PULL_REQUEST_WORKFLOW.contains("uses: ./.github/workflows/build-platforms.yml"));
}

#[test]
fn release_version_is_raised_from_pull_request_labels() {
    for level in ["major", "minor", "patch"] {
        assert!(
            RELEASE_WORKFLOW.contains(&format!("release:{level}")),
            "workflow ignores release:{level}"
        );
        assert!(
            VERSION_BUMPER.contains(&format!("{level})")),
            "bump script ignores {level}"
        );
    }
    assert!(RELEASE_WORKFLOW.contains("./scripts/bump-version.sh"));
    assert!(RELEASE_WORKFLOW.contains("chore(release): v${version}"));
    // Both files carry the version, and CI builds with --locked.
    assert!(VERSION_BUMPER.contains("Cargo.toml"));
    assert!(VERSION_BUMPER.contains("Cargo.lock"));
}
