//! warp_rs —— ExampleRO 客户端补丁引擎（WARP 核心能力的 Rust 重写）。
//!
//! 这个 crate 同时提供两种用法：
//!
//! * **命令行**：`src/main.rs`（`warp_rs apply / verify / find / langs …`）
//! * **库**：`tools/updater_tauri` 通过 `path = "../warp_rs"` 直接依赖本 crate，
//!   在启动器里做「补丁列表 / 勾选 / 参数输入 / 应用 / 还原」，不起子进程。
//!
//! ## 模块一览
//!
//! | 模块 | 作用 |
//! |---|---|
//! | [`pe`] | PE 头解析（位数、段表、编译时间） |
//! | [`engine`] | 在 exe 字节里找特征串 / 读写字（VA ↔ 文件偏移） |
//! | [`hex`] | hex 模式串（`??` 通配）解析与格式化 |
//! | [`encode`] | GBK / CP949 / UTF-8 编码转换 |
//! | [`table`] | 补丁表（YAML）数据结构 |
//! | [`params`] | `ask:` 参数（文本 / 路径 / 文件 / 枚举）解析与校验 |
//! | [`patches`] | `mode: custom` 的 Rust 处理器（各条特殊补丁） |
//! | [`apply`] | 打补丁 / 校验，产出可核验的 JSON 报告 |
//! | [`langs`] | 扫描语言包目录（启动器下拉框用） |
//! | [`session`] | **启动器用**：把多张表合成一个「补丁会话」，按勾选应用 |
//!
//! ## 快速上手（库）
//!
//! ```no_run
//! use warp_rs::session::Session;
//!
//! let mut s = Session::new();
//! s.add_table(
//!     "base",
//!     "base.yml",
//!     "基础",
//!     warp_rs::table::PatchTable::load("patches/base.yml".as_ref())?,
//! );
//!
//! // 勾选 base 表里的两条补丁（id），语言包用 zh-cn
//! let (report, bytes) = s.run(
//!     std::fs::read("Ragexe.exe")?,
//!     &["zoom-max".to_string(), "always-ascii".to_string()],
//!     Some("zh-cn"),
//!     Vec::new(),
//!     true, // dry-run
//! )?;
//! println!("{} 条打上 / {} 条不适用", report.applied, report.not_applicable);
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod apply;
pub mod encode;
pub mod engine;
pub mod hex;
pub mod langs;
pub mod params;
pub mod patches;
pub mod pe;
pub mod session;
pub mod table;

pub use apply::{ApplyOptions, Entry, Report, Status, VerifyOptions};
pub use params::{AskKind, AskSpec, ParamResolver};
pub use table::{PatchSpec, PatchTable, TableMeta};
