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
}
