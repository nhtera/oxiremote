// Build script for the `desktop` subcrate.
//
// Responsibilities:
//   1. macOS Swift rpaths so `cargo test --lib -p desktop` finds
//      `libswift_Concurrency.dylib` at startup (the screencapturekit crate
//      pulls in Swift bridging code).
//   2. When the `vp9` feature is enabled, run `bindgen` against the system
//      libvpx headers (Homebrew `libvpx`, `apt install libvpx-dev`, vcpkg)
//      and emit `$OUT_DIR/vpx_bindings.rs`. Includes the linker hint
//      `cargo:rustc-link-lib=vpx` resolved via `pkg-config`.
//
// `cargo:rustc-link-arg` from a library crate's build.rs is dropped when
// the crate is consumed as an rlib, so the rpath block only applies to
// the test binary — the agent binary picks up the same rpaths from
// `agent/build.rs`.

fn main() {
    // ─── 1. macOS Swift rpaths (test binary) ─────────────────────────────
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for rpath in &[
            "/usr/lib/swift",
            "/Library/Developer/CommandLineTools/usr/lib/swift-5.5/macosx",
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
        ] {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{rpath}");
        }
    }

    // ─── 2. VP9 / libvpx bindgen ─────────────────────────────────────────
    // CARGO_FEATURE_VP9 is set by cargo when the `vp9` feature is active.
    // Non-vp9 builds skip the entire bindgen invocation (and the libvpx
    // system-dep requirement).
    if std::env::var("CARGO_FEATURE_VP9").is_ok() {
        generate_vpx_bindings();
    }

    // ─── 3. AV1 / libaom bindgen ─────────────────────────────────────────
    // CARGO_FEATURE_AV1 is set by cargo when the `av1` feature is active.
    // libaom ≥ 3.0 is required for AV1E_SET_TUNE_CONTENT(AOM_CONTENT_SCREEN);
    // Homebrew + Ubuntu 22.04 + vcpkg all ship 3.x.
    if std::env::var("CARGO_FEATURE_AV1").is_ok() {
        generate_aom_bindings();
    }
}

#[allow(dead_code)] // gated behind CARGO_FEATURE_VP9 at the call site
fn generate_vpx_bindings() {
    use std::path::PathBuf;

    // Resolve libvpx via pkg-config — emits the right `-L` and `-l` flags
    // to the linker AND returns the include paths bindgen needs.
    let vpx = pkg_config::Config::new()
        .atleast_version("1.10")
        .probe("vpx")
        .expect(
            "phase-02: failed to find libvpx via pkg-config. \
             Install with `brew install libvpx` (macOS), \
             `apt install libvpx-dev` (Linux), \
             or vcpkg `libvpx` (Windows). \
             PKG_CONFIG_PATH may also need an export.",
        );

    // Bindgen builder. We restrict the allowlist to `vpx_*` / `VPX_*` /
    // `vp8_*` / `vp9_*` / `VP8_*` / `VP9_*` symbols so the binding file
    // stays small and doesn't drag in system stdlib types.
    let mut builder = bindgen::Builder::default()
        .header_contents(
            "vpx_wrapper.h",
            "#include <vpx/vpx_encoder.h>\n\
             #include <vpx/vp8cx.h>\n\
             #include <vpx/vpx_image.h>\n",
        )
        .allowlist_function("vpx_.*")
        .allowlist_function("vp8_.*")
        .allowlist_function("vp9_.*")
        .allowlist_type("vpx_.*")
        .allowlist_type("vp8.*")
        .allowlist_type("vp9.*")
        .allowlist_type("VP[X89].*")
        .allowlist_var("VPX_.*")
        .allowlist_var("VP8.*")
        .allowlist_var("VP9.*")
        // bindgen turns control IDs (VP8E_SET_CPUUSED etc.) into
        // anonymous-enum constants. Newtype keeps them typed without
        // forcing a single u32.
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        // Suppress Cargo cache invalidation on every system header touch
        // — we explicitly rerun only when our own header wrapper changes.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .layout_tests(false);

    for inc in &vpx.include_paths {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }

    let bindings = builder
        .generate()
        .expect("phase-02: bindgen failed to parse libvpx headers");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("vpx_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("phase-02: failed to write vpx_bindings.rs");

    println!("cargo:rerun-if-changed=build.rs");
}

#[allow(dead_code)] // gated behind CARGO_FEATURE_AV1 at the call site
fn generate_aom_bindings() {
    use std::path::PathBuf;

    let aom = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("aom")
        .expect(
            "phase-05: failed to find libaom via pkg-config. \
             Install with `brew install aom` (macOS), \
             `apt install libaom-dev` (Linux, ≥ Ubuntu 22.04), \
             or vcpkg `aom` (Windows). \
             Requires version ≥ 3.0 for AOM_CONTENT_SCREEN support.",
        );

    let mut builder = bindgen::Builder::default()
        .header_contents(
            "aom_wrapper.h",
            "#include <aom/aom_encoder.h>\n\
             #include <aom/aomcx.h>\n\
             #include <aom/aom_image.h>\n",
        )
        .allowlist_function("aom_.*")
        .allowlist_function("AOM_.*")
        .allowlist_type("aom_.*")
        .allowlist_type("aome_.*")
        .allowlist_type("AOM_.*")
        .allowlist_type("av1.*")
        .allowlist_var("AOM_.*")
        .allowlist_var("AOME_.*")
        .allowlist_var("AV1E_.*")
        // Control-ID enums live under `aome_enc_control_id` / `av1e_enc_*`
        // typedef names. Without explicit allowlist coverage their
        // constants (AOME_SET_CPUUSED, AV1E_SET_TUNE_CONTENT, ...) don't
        // make it into the generated module-consts.
        .allowlist_type("aome_enc.*")
        .allowlist_type("av1e_enc.*")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .layout_tests(false);

    for inc in &aom.include_paths {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }

    let bindings = builder
        .generate()
        .expect("phase-05: bindgen failed to parse libaom headers");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("aom_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("phase-05: failed to write aom_bindings.rs");
}
