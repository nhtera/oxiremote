fn main() {
    println!("cargo:rerun-if-changed=../apps/web/dist");

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" {
        let dist = std::path::Path::new("../apps/web/dist/index.html");
        let assets = std::path::Path::new("../apps/web/dist/assets");
        if !dist.exists() || !assets.exists() {
            panic!(
                "apps/web/dist not found — run `bun run build:web` before release build"
            );
        }
    }

    // Embed Swift runtime rpaths so the screencapturekit crate's Swift
    // bridging code can locate libswift_Concurrency.dylib at startup.
    //
    // macOS 14+ no longer ships these libs in the OS dyld shared cache, and
    // the screencapturekit-rs build script doesn't add a fallback rpath.
    // Both Xcode.app and Command Line Tools install them in known locations;
    // we add both, plus the legacy /usr/lib/swift, and dyld silently skips
    // any path that doesn't exist on a given host. Belongs in the BIN crate
    // (not the desktop lib) because cargo:rustc-link-arg only affects the
    // owning crate's link step.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // ORDER MATTERS. dyld searches rpaths in declaration order; we want
        // the OS-provided Swift libs (resolved from the dyld shared cache via
        // `/usr/lib/swift`) to win, falling back to the developer toolchain's
        // copies only if the OS doesn't ship them. With the CLI Tools path
        // listed FIRST we observed duplicate-class loads (both copies present
        // in the address space) which dyld warns about as "may cause spurious
        // casting failures and mysterious crashes".
        for rpath in &[
            "/usr/lib/swift",
            "/Library/Developer/CommandLineTools/usr/lib/swift-5.5/macosx",
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
        ] {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{rpath}");
        }
    }
}
