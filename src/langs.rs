//! `warp_rs langs` —— 扫描语言包目录（启动器下拉框用）。
//!
//! 语言表统一放 `patches/lang/`，每张表头带 `meta`：
//!
//! ```yaml
//! meta:
//!   lang: zh-cn
//!   name: 简体中文
//!   encoding: gbk        # 写进 exe 的字节编码
//! version: 1
//! patches: [...]
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::Serialize;

use crate::table::PatchTable;

/// 一张语言表的信息。
#[derive(Debug, Clone, Serialize)]
pub struct LangInfo {
    /// 文件名（相对目录）。
    pub file: String,
    /// 语言代码（`meta.lang`）。
    pub lang: String,
    /// 语言名（`meta.name`）。
    pub name: String,
    /// 落地编码（`meta.encoding`，没有就按默认 gbk）。
    pub encoding: String,
    /// 补丁条数。
    pub count: usize,
    /// 解析错误（表坏了也列出来，方便发现）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 扫描目录里的 `*.yml` / `*.yaml`。
pub fn scan(dir: &Path) -> Result<Vec<LangInfo>> {
    if !dir.exists() {
        bail!("语言包目录不存在：{}", dir.display());
    }
    if !dir.is_dir() {
        bail!("语言包路径不是目录：{}", dir.display());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|e| e == "yml" || e == "yaml")
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    Ok(files.iter().map(|f| info_of(f)).collect())
}

/// 读一张语言表的信息。
pub fn info_of(path: &Path) -> LangInfo {
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match PatchTable::load(path) {
        Ok(t) => {
            let meta = t.meta.clone().unwrap_or_default();
            LangInfo {
                file,
                lang: meta.lang.clone().unwrap_or_default(),
                name: meta.name.clone().unwrap_or_default(),
                encoding: meta.encoding.clone().unwrap_or_else(|| "gbk".to_string()),
                count: t.patches.len(),
                error: None,
            }
        }
        Err(e) => LangInfo {
            file,
            lang: String::new(),
            name: String::new(),
            encoding: String::new(),
            count: 0,
            error: Some(format!("{e:#}")),
        },
    }
}

/// 显示宽度（中文算 2 列）。
fn width(s: &str) -> usize {
    s.chars()
        .map(|c| if ('\u{1100}'..='\u{115F}').contains(&c) || ('\u{2E80}'..='\u{A4CF}').contains(&c)
            || ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{F900}'..='\u{FAFF}').contains(&c)
            || ('\u{FE30}'..='\u{FE6F}').contains(&c) || ('\u{FF00}'..='\u{FF60}').contains(&c)
            || ('\u{FFE0}'..='\u{FFE6}').contains(&c) {
                2
            } else {
                1
            })
        .sum()
}

/// 左对齐补空格（按显示宽度）。
fn pad(s: &str, w: usize) -> String {
    let n = width(s);
    format!("{}{}", s, " ".repeat(w.saturating_sub(n)))
}

/// 打成表格（人看）。
pub fn format_table(dir: &Path, list: &[LangInfo]) -> String {
    let mut out = String::new();
    out.push_str(&format!("语言包目录：{}\n", dir.display()));
    out.push_str(&format!(
        "{} {} {}  {}\n",
        pad("lang", 12),
        pad("name", 16),
        format!("{:>4}", "条目"),
        "编码"
    ));
    for l in list {
        if let Some(e) = &l.error {
            out.push_str(&format!(
                "{} {} {}  {}\n",
                pad("-", 12),
                pad("-", 16),
                format!("{:>4}", "-"),
                e
            ));
            continue;
        }
        out.push_str(&format!(
            "{} {} {}  {}\n",
            pad(&l.lang, 12),
            pad(&l.name, 16),
            format!("{:>4}", l.count),
            l.encoding
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 扫目录() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/lang");
        if !dir.exists() {
            return;
        }
        let list = scan(&dir).unwrap();
        assert!(!list.is_empty());
        assert!(list.iter().any(|l| l.lang == "zh-cn"));
    }
}
