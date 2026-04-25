// `cargo test --lib -p desktop` produces its own test binary that needs to
// locate libswift_Concurrency.dylib at startup (pulled in by the
// screencapturekit crate's Swift bridging code on macOS).
//
// `cargo:rustc-link-arg` from a library crate's build.rs is dropped when the
// crate is consumed as an rlib, so this only applies to the test binary —
// the agent binary picks up the same rpaths from `agent/build.rs`. Keeping
// both means `cargo test --lib -p desktop` passes without
// `DYLD_FALLBACK_LIBRARY_PATH` set.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for rpath in &[
            "/usr/lib/swift",
            "/Library/Developer/CommandLineTools/usr/lib/swift-5.5/macosx",
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
        ] {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{rpath}");
        }
    }
}
