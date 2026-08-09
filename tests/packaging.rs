const INFO_PLIST: &str = include_str!("../packaging/Info.plist");
const ICON_SVG: &str = include_str!("../packaging/Textify.svg");
const ICON_ICNS: &[u8] = include_bytes!("../packaging/Textify.icns");

#[test]
fn macos_bundle_declares_the_textify_icon_resource() {
    assert!(INFO_PLIST.contains("<key>CFBundleIconFile</key>"));
    assert!(INFO_PLIST.contains("<string>Textify.icns</string>"));
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
