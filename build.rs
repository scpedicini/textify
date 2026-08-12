fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/Textify.rc");
    println!("cargo:rerun-if-changed=packaging/windows/Textify.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("packaging/windows/Textify.rc", embed_resource::NONE)
            .manifest_required()
            .expect("could not embed the Textify Windows icon");
    }
}
