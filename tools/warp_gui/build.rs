fn main() {
    // 只有开了 `gui` feature 才需要生成 Tauri 上下文（否则纯逻辑构建不需要图形库）
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build();
    }

    // 仓库里的补丁表目录：改了 patches/*.yml 就重新编译
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let patches = manifest.join("../../patches");
    println!("cargo:rerun-if-changed={}", patches.display());
}
