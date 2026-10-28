//! `warp-gui` 可执行入口。
//!
//! * 开 `gui` feature（`cargo build --features gui`）：起 Tauri 窗口。
//!   Windows 链 WebView2（系统自带），Linux 链 webkit2gtk-4.1。
//! * 不开（默认）：只编译逻辑库与这个占位壳，任何平台都不需要图形系统库，
//!   方便 `cargo test` / CI 里的纯逻辑校验。
//!
//! 界面层（`ui/`）是纯静态的三个文件，不引入 node / vite 构建链；
//! 后端命令在 `src/commands.rs`。

// Windows 图形程序不弹控制台窗口（和 updater_tauri 一致）
#![cfg_attr(all(feature = "gui", not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
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
        println!("不带参数 = 打开图形界面（需要 --features gui 构建）。");
        return;
    }

    #[cfg(feature = "gui")]
    commands::run();

    #[cfg(not(feature = "gui"))]
    {
        println!("本二进制没有编入图形界面（构建时未开 `gui` feature）。");
        println!("发行版请用：cargo build --release --features gui");
        println!("纯逻辑校验：cargo test");
    }
}
