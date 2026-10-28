//! `warp-gui` 可执行入口。
//!
//! * **Windows**：起 Tauri 窗口（WebView2，Win10/11 自带）。
//! * **其它平台**：打印一行提示后正常退出（退出码 0）——
//!   GUI 只在 Windows 目标下编译，但纯逻辑层（`warp_gui` 库）在哪都能跑
//!   （`cargo test` 覆盖扫描 / plan / 备份命名等）。
//!
//! 界面层（`ui/`）是纯静态的三个文件，不引入 node / vite 构建链；
//! 后端命令在 `src/commands.rs`。

// Windows 图形程序不弹控制台窗口（和 updater_tauri 一致）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 只有 Windows 目标才编译命令层（它依赖 tauri）
#[cfg(target_os = "windows")]
mod commands;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // --version / --help：不进图形界面（给 CI 烟测用）
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("warp-gui {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("用法：warp-gui [--version | --help]");
        println!("不带参数 = 打开图形界面（仅 Windows 构建）。");
        return;
    }

    #[cfg(target_os = "windows")]
    commands::run();

    #[cfg(not(target_os = "windows"))]
    {
        println!("GUI 仅在 Windows 构建");
        println!("（当前平台的 warp_gui 只提供逻辑库：`cargo test` 可跑全部单测）");
    }
}
