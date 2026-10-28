//! 补丁表（YAML）的数据结构。
//!
//! ```yaml
//! version: 1
//! patches:
//!   - id: translate_msgbox          # 唯一标识
//!     desc: 消息框标题               # 中文说明（之后显示在启动器界面里）
//!     find: "B8 DE BD C3 C1 F6 00"  # hex，空格分隔，?? 为通配
//!     replace: "CF FB CF A2"        # hex（GBK「消息」）
//!     section: .data                # 可选，限定段
//!     expect: 1                     # 期望命中次数，不符即失败
//!     mode: replace                 # replace | nop | jmp
//!     dates: { start: 20100000, end: 20991231 }
//! ```

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::params::AskSpec;

/// 表头元信息（语言包用）。
///
/// ```yaml
/// meta:
///   lang: zh-cn          # 语言代码
///   name: 简体中文        # 语言名（启动器下拉框显示）
///   encoding: gbk        # 写进 exe 的字节编码
/// ```
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TableMeta {
    /// 语言代码（zh-cn / en / ja …）。
    #[serde(default)]
    pub lang: Option<String>,
    /// 语言名（显示用）。
    #[serde(default)]
    pub name: Option<String>,
    /// 写进 exe 的字节编码（gbk / cp949 / utf8 / sjis / cp874）。
    #[serde(default)]
    pub encoding: Option<String>,
}

impl TableMeta {
    /// 表头声明的编码（没有就 None）。
    pub fn encoding(&self) -> Option<&str> {
        self.encoding.as_deref()
    }
}

/// 补丁表。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchTable {
    /// 表格式版本，目前是 1。
    #[serde(default)]
    pub version: u32,
    /// 表头元信息（语言包用；普通补丁表可以没有）。
    #[serde(default)]
    pub meta: Option<TableMeta>,
    /// 补丁条目。
    #[serde(default)]
    pub patches: Vec<PatchSpec>,
}

impl PatchTable {
    /// 从 YAML 文本解析。
    pub fn from_yaml(text: &str) -> Result<PatchTable> {
        let table: PatchTable = serde_yaml::from_str(text)?;
        if table.version > 1 {
            bail!(
                "补丁表版本 {} 太新（本工具只支持 version <= 1）",
                table.version
            );
        }
        Ok(table)
    }

    /// 读文件并解析。
    pub fn load(path: &std::path::Path) -> Result<PatchTable> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读补丁表 {} 失败：{e}", path.display()))?;
        PatchTable::from_yaml(&text)
    }
}

/// 补丁模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 原地覆盖（默认）。
    Replace,
    /// 写 N 个 `90`。
    Nop,
    /// 写一条相对跳转。
    Jmp,
    /// 走 Rust 侧的自定义处理器（`handler:` 字段指定处理器名）。
    Custom,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "replace" | "patch" | "set" => Ok(Mode::Replace),
            "nop" | "nops" => Ok(Mode::Nop),
            "jmp" | "jump" => Ok(Mode::Jmp),
            "custom" | "handler" | "hook" => Ok(Mode::Custom),
            other => bail!("未知补丁模式：{other}（支持 replace / nop / jmp）"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Mode::Replace => "replace",
            Mode::Nop => "nop",
            Mode::Jmp => "jmp",
            Mode::Custom => "custom",
        }
    }
}

/// 编译时间过滤区间（YYYYMMDD 整数；也接受 unix 时间戳）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DateRange {
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub end: Option<u32>,
}

/// 一条补丁。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchSpec {
    /// 唯一标识。
    pub id: String,
    /// 中文说明（界面显示：「中文（English）」）。
    #[serde(default)]
    pub desc: Option<String>,
    /// 英文原文。
    #[serde(default)]
    pub desc_en: Option<String>,
    /// 搜索模式（hex，支持 `??` 通配）。`mode: custom` 时可以省略。
    #[serde(default)]
    pub find: String,
    /// 替换字节（hex）。
    #[serde(default)]
    pub replace: Option<String>,
    /// 替换文本（和 `encoding` 一起用）。
    #[serde(default)]
    pub text: Option<String>,
    /// 文本编码：gbk / cp949 / utf8。
    #[serde(default)]
    pub encoding: Option<String>,
    /// 限定搜索段（如 `.data`）。
    #[serde(default)]
    pub section: Option<String>,
    /// 期望命中次数（不符即失败）。默认 1。
    #[serde(default)]
    pub expect: Option<usize>,
    /// 补丁模式，默认 `replace`。
    #[serde(default)]
    pub mode: Option<String>,
    /// 编译时间过滤。
    #[serde(default)]
    pub dates: Option<DateRange>,
    /// 打第 N 次命中（1 开始）。默认 1。
    #[serde(default)]
    pub nth: Option<usize>,
    /// 打全部命中。
    #[serde(default)]
    pub all: bool,
    /// 打最后一次命中。
    #[serde(default)]
    pub last: bool,
    /// 替换后是否补一个 `00` 结束符（字符串补丁用）。
    #[serde(default)]
    pub terminate: Option<bool>,
    /// `mode: nop` 时的 NOP 个数，默认等于 find 的长度。
    #[serde(default)]
    pub nop_len: Option<usize>,
    /// `mode: jmp` 时的目标 VA。
    #[serde(default)]
    pub target: Option<u64>,
    /// 优先级大的先打（重叠时用来决定谁生效）。
    #[serde(default)]
    pub priority: Option<i32>,
    /// 标记为不启用（引擎会跳过并写进报告）。
    #[serde(default)]
    pub disabled: bool,
    /// 备注 / 原因。
    #[serde(default)]
    pub note: Option<String>,
    /// `mode: custom` 时指定用哪个 Rust 处理器（见 `src/patches/`）。
    #[serde(default)]
    pub handler: Option<String>,
    /// 参数声明（对应 WARP 的 GetUserInput 弹窗）。
    #[serde(default)]
    pub ask: Option<AskSpec>,
    /// 替换字节比 find 短时，把 find 剩下的字节清零（原地覆写不留残渣）。
    #[serde(default)]
    pub zero_fill: Option<bool>,
    /// 原路径特征（`custom-lub` 处理器用）：在段里找匹配这个模式的 C 字符串。
    ///
    /// * `*` 是通配（匹配任意长度）。
    /// * **大小写不敏感**。
    ///
    /// 例：`system\\OngoingQuestInfoList*` 能同时命中 `..._Sakray` 和 `..._True`。
    #[serde(default)]
    pub path_old: Option<String>,
}

impl PatchSpec {
    /// 补丁模式。
    pub fn mode(&self) -> Result<Mode> {
        match &self.mode {
            Some(m) => Mode::parse(m),
            None => Ok(Mode::Replace),
        }
    }

    /// 期望命中次数（默认 1）。
    pub fn expect(&self) -> usize {
        self.expect.unwrap_or(1)
    }

    /// 优先级（默认 0）。
    pub fn priority(&self) -> i32 {
        self.priority.unwrap_or(0)
    }
}
