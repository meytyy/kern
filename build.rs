//! Windows resources (icon, version) for KERN.exe. Nothing to do on other targets.
fn main() {
    println!("cargo:rerun-if-changed=assets/windows/kern.rc");
    println!("cargo:rerun-if-changed=assets/windows/kern.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows/kern.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
