fn main() {
    windows_icon();
    macos_sparkle();
}

/// The Windows executable carries the app icon as its resource 1, which GPUI loads for its window
/// class. The `.ico` is checked on every host, including CI without a Windows toolchain.
fn windows_icon() {
    let _ = include_bytes!("../../assets/icons/Sakura.ico");
    println!("cargo:rerun-if-changed=../../assets/icons/Sakura.ico");
    println!("cargo:rerun-if-changed=windows.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("windows.rc", ["sakura"], embed_resource::NONE)
            .manifest_required()
            .expect("the Windows executable must carry its icon resource");
    }
}

/// On the Mac the binary links Sparkle weakly from the bundle's Frameworks folder, when the build
/// names the framework's folder: a bare binary stays launchable without it.
fn macos_sparkle() {
    println!("cargo:rerun-if-env-changed=SAKURA_SPARKLE_FRAMEWORK_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    if let Ok(directory) = std::env::var("SAKURA_SPARKLE_FRAMEWORK_DIR") {
        println!("cargo:rustc-link-search=framework={directory}");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,Sparkle");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
}
