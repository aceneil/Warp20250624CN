use std::path::PathBuf;

fn main() {
    // 只在目标平台为 Windows 时生成 Tauri 上下文（Linux 上编译命令行版，无需 webkit）
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        tauri_build::build();
    }

    // 仓库里的补丁表目录：改了 patches/*.yml 就重新编译
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let patches = manifest.join("../../patches");
    println!("cargo:rerun-if-changed={}", patches.display());
}
