//! Tauri 命令层：只做「参数校验 + 转调 `warp_gui` 逻辑」，不含业务逻辑。
//!
//! 前端拿到的错误都是字符串，界面上直接显示，不让 panic 冒到 WebView。
//!
//! 命令一览：
//!
//! | 命令 | 作用 |
//! |---|---|
//! | `cmd_pick_exe` | 「浏览…」：弹原生文件对话框挑客户端 exe |
//! | `cmd_exe_info` | 读客户端 exe 的文件名 / 大小 / md5 |
//! | `cmd_scan_tables` | 扫描补丁表目录，列出 `patches/*.yml` |
//! | `cmd_table_entries` | 解析一张表，逐条算「在当前 exe 上能不能定位」 |
//! | `cmd_plan` | 预览（dry-run）：只报告，不写文件 |
//! | `cmd_apply` | 应用：备份 → 打补丁 → 写 `<原名>_patched.exe` → 校验 |
//! | `cmd_verify` | 校验一个已经打过补丁的 exe |

#![cfg(target_os = "windows")]

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use warp_gui::{
    apply_patch, find_patches_dir, load_table, plan, scan, verify_patch, ApplyResult, ExeInfo,
    PlanResult, ScanResult, TableView, VerifyResult,
};

/// 补丁表目录（找不到就报错，界面上给一行中文说明）。
fn patches_dir() -> Result<PathBuf> {
    find_patches_dir().ok_or_else(|| {
        anyhow!(
            "找不到补丁表目录（patches/）：请把补丁表放在程序旁边，或设置环境变量 WARP_GUI_PATCHES_DIR"
        )
    })
}

/// 补丁表路径：只允许目录里的文件名（挡掉 `../` 之类的路径穿越）。
fn table_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let p = Path::new(name);
    let same = p
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .as_deref()
        == Some(name);
    if !same || name.trim().is_empty() {
        bail!("补丁表名不合法：{name}");
    }
    Ok(dir.join(name))
}

/// 「浏览…」：弹原生文件对话框挑一个客户端 exe。
///
/// 取消选择返回 `None`（界面不报错，只是没有新的选择）。
#[tauri::command]
pub async fn cmd_pick_exe(app: AppHandle) -> Result<Option<String>, String> {
    // 对话框是阻塞的，不能占着主线程 → 丢到阻塞线程池里
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("客户端程序", &["exe"])
            .set_title("选择客户端 exe")
            .blocking_pick_file()
            .map(|p| p.to_string())
    })
    .await
    .map_err(|e| format!("打开文件对话框失败：{e}"))
}

/// 读客户端 exe 的信息（文件名 / 大小 / md5）。
#[tauri::command]
pub fn cmd_exe_info(path: String) -> Result<ExeInfo, String> {
    warp_gui::exe_info(Path::new(&path)).map_err(|e| format!("{e:#}"))
}

/// 扫描补丁表目录。
#[tauri::command]
pub fn cmd_scan_tables() -> Result<ScanResult, String> {
    let dir = patches_dir().map_err(|e| format!("{e:#}"))?;
    scan(&dir).map_err(|e| format!("{e:#}"))
}

/// 解析一张补丁表，逐条算可用性（`exe` 可以为空 = 还没选客户端）。
#[tauri::command]
pub fn cmd_table_entries(table: String, exe: Option<String>) -> Result<TableView, String> {
    let dir = patches_dir().map_err(|e| format!("{e:#}"))?;
    let path = table_path(&dir, &table).map_err(|e| format!("{e:#}"))?;
    let exe = exe.filter(|s| !s.trim().is_empty()).map(PathBuf::from);
    load_table(&path, exe.as_deref()).map_err(|e| format!("{e:#}"))
}

/// 预览（dry-run）：只算 plan，不写任何文件。
#[tauri::command]
pub async fn cmd_plan(exe: String, table: String, ids: Vec<String>) -> Result<PlanResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = patches_dir()?;
        let path = table_path(&dir, &table)?;
        plan(Path::new(&exe), &path, &ids)
    })
    .await
    .map_err(|e| format!("预览任务失败：{e}"))?
    .map_err(|e| format!("{e:#}"))
}

/// 应用：备份 → 打补丁 → 写 `<原名>_patched.exe` → 校验。
#[tauri::command]
pub async fn cmd_apply(exe: String, table: String, ids: Vec<String>) -> Result<ApplyResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = patches_dir()?;
        let path = table_path(&dir, &table)?;
        apply_patch(Path::new(&exe), &path, &ids)
    })
    .await
    .map_err(|e| format!("应用任务失败：{e}"))?
    .map_err(|e| format!("{e:#}"))
}

/// 校验一个已经打过补丁的 exe。
#[tauri::command]
pub async fn cmd_verify(exe: String, table: String, ids: Vec<String>) -> Result<VerifyResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = patches_dir()?;
        let path = table_path(&dir, &table)?;
        // 优先校验打过补丁的那个（`<原名>_patched.exe`），没有就校验原 exe
        let target = warp_gui::verify_target(Path::new(&exe));
        verify_patch(&target, &path, &ids)
    })
    .await
    .map_err(|e| format!("校验任务失败：{e}"))?
    .map_err(|e| format!("{e:#}"))
}

/// 起 Tauri 窗口。
pub fn run() {
    tauri::Builder::default()
        // 原生「打开文件」对话框（「浏览…」按钮）
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            cmd_pick_exe,
            cmd_exe_info,
            cmd_scan_tables,
            cmd_table_entries,
            cmd_plan,
            cmd_apply,
            cmd_verify,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 窗口失败");
}
