//! The build id for About, and the Windows resources (icon, version) of KERN.exe.
fn main() {
    // The short commit for Help → About; "local" outside a Git checkout.
    let build = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "local".into());
    println!("cargo:rustc-env=KERN_BUILD={build}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-changed=assets/windows/kern.rc");
    println!("cargo:rerun-if-changed=assets/windows/kern.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows/kern.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
