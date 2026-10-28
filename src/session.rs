//! 启动器用的「补丁会话」——把多张补丁表合成一份，按勾选结果应用。
//!
//! 启动器（`tools/updater_tauri`）的补丁页需要：
//!
//! * 列出所有表（分组：基础 / 可选 / 自定义路径……），每条补丁带 `desc` / `desc_en` / `ask`
//! * 用户勾选若干条 + 填参数 + 选语言包
//! * 预览（dry-run）/ 应用 / 还原
//!
//! 这里把「合成一张表」和「跑一次 apply」包起来，界面层只管勾选和显示。
//!
//! ## 为什么合成一张表而不是一张张打
//!
//! `apply` 内部会做**冲突检测**（两条补丁写同一块 → 后者跳过），
//! 合成一张表时这个检测是跨表生效的；分多次打则检测不到跨表冲突。
//! 另外语言包要和功能补丁在同一次落盘里写，顺序才不会乱。

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;

use crate::apply::{self, ApplyOptions, Report};
use crate::params::{AskKind, AskSpec, ParamResolver};
use crate::table::{PatchSpec, PatchTable, TableMeta};

/// 一条补丁在界面上的展示信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    /// 补丁 id（唯一）。
    pub id: String,
    /// 中文说明。
    pub desc: Option<String>,
    /// 英文原文（界面上显示成灰色小字）。
    pub desc_en: Option<String>,
    /// 参数声明（`ask:`）。没有就是 None。
    pub ask: Option<AskInfo>,
    /// 补丁模式：replace / nop / jmp / custom。
    pub mode: String,
}

/// `ask:` 参数的展示信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskInfo {
    /// 参数名。
    pub key: String,
    /// 提示语。
    pub label: Option<String>,
    /// 默认值。
    pub default: Option<String>,
    /// 参数种类：text / path / file / enum。
    pub kind: String,
    /// 字符数下限。
    pub min: Option<usize>,
    /// 字符数上限。
    pub max: Option<usize>,
    /// 候选值（`kind: enum`）。
    pub choices: Option<Vec<String>>,
}

/// 一张补丁表（界面上的一个分组）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    /// 表的唯一标识（勾选状态按它 + 补丁 id 记录）。
    pub key: String,
    /// 来源文件名（显示用）。
    pub file: String,
    /// 分组标题（基础 / 可选 / 自定义路径……）。
    pub group: String,
    /// 表里的补丁。
    pub entries: Vec<EntryInfo>,
}

/// 一张语言表（界面上的语言下拉框）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LangInfo {
    /// 语言代码（`meta.lang`）。
    pub lang: String,
    /// 语言名（`meta.name`）。
    pub name: String,
    /// 落地编码（`meta.encoding`）。
    pub encoding: String,
    /// 补丁条数。
    pub count: usize,
}

/// 会话里的一张功能补丁表。
#[derive(Debug, Clone)]
pub struct TableEntry {
    /// 唯一标识。
    pub key: String,
    /// 来源文件名。
    pub file: String,
    /// 分组标题。
    pub group: String,
    /// 表内容。
    pub table: PatchTable,
}

/// 会话里的一张语言表。
#[derive(Debug, Clone)]
pub struct LangEntry {
    /// 语言代码（下拉框的 value）。
    pub lang: String,
    /// 语言名。
    pub name: String,
    /// 落地编码。
    pub encoding: String,
    /// 表内容。
    pub table: PatchTable,
}

/// 一个补丁会话：若干张功能补丁表 + 若干张语言表。
#[derive(Debug, Clone, Default)]
pub struct Session {
    tables: Vec<TableEntry>,
    langs: Vec<LangEntry>,
}

impl Session {
    /// 空的会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 加一张功能补丁表。
    ///
    /// * `key`   —— 唯一标识（同一张表重复加入时后来的覆盖先来的）
    /// * `file`  —— 来源文件名（显示用）
    /// * `group` —— 分组标题（基础 / 可选 / 自定义路径……）
    pub fn add_table(&mut self, key: &str, file: &str, group: &str, table: PatchTable) {
        let entry = TableEntry {
            key: key.to_string(),
            file: file.to_string(),
            group: group.to_string(),
            table,
        };
        match self.tables.iter_mut().find(|t| t.key == entry.key) {
            Some(slot) => *slot = entry,
            None => self.tables.push(entry),
        }
    }

    /// 加一张语言表。
    pub fn add_lang(&mut self, lang: &str, name: &str, encoding: &str, table: PatchTable) {
        let entry = LangEntry {
            lang: lang.to_string(),
            name: name.to_string(),
            encoding: encoding.to_string(),
            table,
        };
        match self.langs.iter_mut().find(|l| l.lang == entry.lang) {
            Some(slot) => *slot = entry,
            None => self.langs.push(entry),
        }
    }

    /// 所有的功能补丁表。
    pub fn tables(&self) -> &[TableEntry] {
        &self.tables
    }

    /// 所有的语言表。
    pub fn lang_entries(&self) -> &[LangEntry] {
        &self.langs
    }

    /// 界面上要显示的分组列表（按加入顺序）。
    pub fn groups(&self) -> Vec<GroupInfo> {
        self.tables
            .iter()
            .map(|t| GroupInfo {
                key: t.key.clone(),
                file: t.file.clone(),
                group: t.group.clone(),
                entries: t.table.patches.iter().map(entry_of).collect(),
            })
            .collect()
    }

    /// 界面上要显示的语言列表。
    pub fn lang_list(&self) -> Vec<LangInfo> {
        self.langs
            .iter()
            .map(|l| LangInfo {
                lang: l.lang.clone(),
                name: l.name.clone(),
                encoding: l.encoding.clone(),
                count: l.table.patches.len(),
            })
            .collect()
    }

    /// 某个补丁 id 属于哪张表（返回分组标题）。
    pub fn group_of(&self, id: &str) -> Option<&str> {
        self.tables
            .iter()
            .find(|t| t.table.patches.iter().any(|p| p.id == id))
            .map(|t| t.group.as_str())
    }

    /// 把勾选的补丁 + 选中的语言包合成一张表。
    ///
    /// * `ids`    —— 勾选的补丁 id（顺序无所谓，按表顺序落）
    /// * `lang`   —— 语言代码（`None` = 不套语言包）
    pub fn merged(&self, ids: &[String], lang: Option<&str>) -> Result<PatchTable> {
        let want: BTreeSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut patches: Vec<PatchSpec> = Vec::new();
        let mut base_enc: Option<String> = None;

        for t in &self.tables {
            let enc = t
                .table
                .meta
                .as_ref()
                .and_then(|m| m.encoding.clone())
                .unwrap_or_else(|| "gbk".to_string());
            if base_enc.is_none() {
                base_enc = Some(enc.clone());
            }
            for spec in &t.table.patches {
                if !want.contains(spec.id.as_str()) {
                    continue;
                }
                let mut spec = spec.clone();
                // 合并后的表级编码取第一张表；别的表编码不一样时，逐条覆盖
                if spec.encoding.is_none() && Some(&enc) != base_enc.as_ref() {
                    spec.encoding = Some(enc.clone());
                }
                patches.push(spec);
            }
        }

        // 语言包排在最后：它改的是字符串表，和功能补丁基本不重叠
        if let Some(code) = lang {
            let code = code.trim();
            if !code.is_empty() {
                let l = self
                    .langs
                    .iter()
                    .find(|l| l.lang == code)
                    .with_context(|| format!("没有这个语言包：{code}"))?;
                let enc = l
                    .table
                    .meta
                    .as_ref()
                    .and_then(|m| m.encoding.clone())
                    .unwrap_or_else(|| l.encoding.clone());
                for spec in &l.table.patches {
                    let mut spec = spec.clone();
                    if spec.encoding.is_none() && Some(&enc) != base_enc.as_ref() {
                        spec.encoding = Some(enc.clone());
                    }
                    patches.push(spec);
                }
            }
        }

        Ok(PatchTable {
            version: 1,
            meta: Some(TableMeta {
                lang: None,
                name: Some("合并补丁".to_string()),
                encoding: base_enc.or(Some("gbk".to_string())),
            }),
            patches,
        })
    }

    /// 跑一次打补丁（`dry_run` 为真时不写文件）。
    ///
    /// * `data`   —— 输入 exe 的字节（调用方决定喂 pristine 还是当前 exe）
    /// * `ids`    —— 勾选的补丁 id
    /// * `lang`   —— 语言包代码（`None` = 不套）
    /// * `params` —— 参数（`ask.key` 或补丁 id → 值）
    /// * `dry_run`—— 只报告不写文件
    pub fn run(
        &self,
        data: Vec<u8>,
        ids: &[String],
        lang: Option<&str>,
        params: Vec<(String, String)>,
        dry_run: bool,
    ) -> Result<(Report, Option<Vec<u8>>)> {
        let table = self.merged(ids, lang)?;
        if table.patches.is_empty() {
            bail!("没有勾选任何补丁");
        }
        let mut opts = ApplyOptions {
            out: None,
            dry_run,
            allow_partial: false,
            params: ParamResolver::new(params, false),
            encoding: None,
        };
        let (rep, bytes) = apply::apply(data, &table, &mut opts)?;
        Ok((rep, bytes))
    }
}

/// 一条补丁的展示信息。
fn entry_of(spec: &PatchSpec) -> EntryInfo {
    EntryInfo {
        id: spec.id.clone(),
        desc: spec.desc.clone(),
        desc_en: spec.desc_en.clone(),
        ask: spec.ask.as_ref().map(ask_of),
        mode: spec.mode().map(|m| m.name().to_string()).unwrap_or_else(|_| "replace".into()),
    }
}

/// `ask:` 的展示信息。
fn ask_of(ask: &AskSpec) -> AskInfo {
    AskInfo {
        key: ask.key.clone(),
        label: ask.label.clone(),
        default: ask.default.clone(),
        kind: match ask.kind {
            AskKind::Text => "text",
            AskKind::Path => "path",
            AskKind::File => "file",
            AskKind::Enum => "enum",
        }
        .to_string(),
        min: ask.min,
        max: ask.max,
        choices: ask.choices.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::PatchTable;

    fn t(yaml: &str) -> PatchTable {
        PatchTable::from_yaml(yaml).unwrap()
    }

    #[test]
    fn 合并按表顺序() {
        let mut s = Session::new();
        s.add_table(
            "base",
            "base.yml",
            "基础",
            t("version: 1\nmeta: {encoding: gbk}\npatches:\n  - {id: a, desc: A}\n  - {id: b, desc: B}\n"),
        );
        s.add_table(
            "opt",
            "optional.yml",
            "可选",
            t("version: 1\nmeta: {encoding: gbk}\npatches:\n  - {id: c, desc: C}\n"),
        );
        let m = s
            .merged(&["c".to_string(), "a".to_string()], None)
            .unwrap();
        let ids: Vec<&str> = m.patches.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn 语言包排在最后() {
        let mut s = Session::new();
        s.add_table(
            "base",
            "base.yml",
            "基础",
            t("version: 1\nmeta: {encoding: gbk}\npatches:\n  - {id: a}\n"),
        );
        s.add_lang(
            "zh-cn",
            "简体中文",
            "gbk",
            t("version: 1\nmeta: {lang: zh-cn, name: 简体中文, encoding: gbk}\npatches:\n  - {id: z1}\n"),
        );
        let m = s.merged(&["a".to_string()], Some("zh-cn")).unwrap();
        let ids: Vec<&str> = m.patches.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "z1"]);
    }

    #[test]
    fn 编码不同的表逐条覆盖() {
        let mut s = Session::new();
        s.add_table(
            "base",
            "base.yml",
            "基础",
            t("version: 1\nmeta: {encoding: gbk}\npatches:\n  - {id: a}\n"),
        );
        s.add_table(
            "cn",
            "cn.yml",
            "中文",
            t("version: 1\nmeta: {encoding: cp949}\npatches:\n  - {id: b}\n"),
        );
        let m = s.merged(&["a".to_string(), "b".to_string()], None).unwrap();
        assert_eq!(m.meta.as_ref().unwrap().encoding.as_deref(), Some("gbk"));
        assert_eq!(m.patches[0].encoding, None);
        assert_eq!(m.patches[1].encoding.as_deref(), Some("cp949"));
    }

    #[test]
    fn 分组信息带参数() {
        let mut s = Session::new();
        s.add_table(
            "base",
            "base.yml",
            "基础",
            t(
                "version: 1\nmeta: {encoding: gbk}\npatches:\n  - id: t\n    desc: 标题\n    desc_en: Title\n    ask:\n      key: title\n      label: 请输入标题\n      default: Ragnarok\n      kind: text\n      min: 1\n      max: 60\n",
            ),
        );
        let g = s.groups();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].group, "基础");
        let a = g[0].entries[0].ask.as_ref().unwrap();
        assert_eq!(a.key, "title");
        assert_eq!(a.kind, "text");
        assert_eq!(a.min, Some(1));
        assert_eq!(a.max, Some(60));
    }
}
