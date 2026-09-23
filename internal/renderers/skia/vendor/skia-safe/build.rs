fn main() {
    println!("cargo:rerun-if-changed=coretext-poc");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("coretext-poc/fontmgr.cpp")
        .include("coretext-poc/skia")
        .define("NDEBUG", None)
        .flag("-fno-rtti")
        .flag("-fno-exceptions")
        .warnings(false)
        .compile("slint-coretext-fontmgr");
    println!("cargo:rustc-link-lib=framework=CoreText");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
