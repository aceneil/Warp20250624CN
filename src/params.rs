//! 补丁表参数（`ask:`）—— 对应 WARP 的 `Exe.GetUserInput()`。
//!
//! WARP 打补丁时会弹窗让用户填值（窗口标题、翻译表路径……）。
//! 本模块把这套「弹窗」抽象成补丁表里的一个 `ask:` 声明：
//!
//! ```yaml
//!   - id: window_title
//!     ask:
//!       key: title            # 参数名，模板里用 {{title}} 引用
//!       label: "请输入新的窗口标题"
//!       default: "Ragnarok"   # 没给值时的默认值
//!       kind: text            # text | path | file | enum
//!       min: 1                # 字符数下限
//!       max: 60               # 字符数上限
//!     find: "52 61 67 6E 61 72 6F 6B 00 00 00 00"
//!     replace: "{{title}}"
//! ```
//!
//! ## 取值优先级
//!
//! ```text
//! 1) --set key=value      （命令行，最高）
//! 2) --interactive         （交互式问用户）
//! 3) 补丁表里的 default
//! 4) 都没有 → 报错（绝不猜）
//! ```
//!
//! 每个值都会记下**来源**（cli / ask / default），写进 JSON 报告，便于复现。

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::encode::{self, Encoding};
use crate::hex;

/// 参数的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AskKind {
    /// 普通文本。
    #[default]
    Text,
    /// 路径（不要求存在）。
    Path,
    /// 文件路径（要求存在）。
    File,
    /// 候选列表（`choices:`）。
    Enum,
}

impl AskKind {
    /// 类型名（提示里显示）。
    pub fn name(&self) -> &'static str {
        match self {
            AskKind::Text => "文本",
            AskKind::Path => "路径",
            AskKind::File => "文件（必须存在）",
            AskKind::Enum => "枚举",
        }
    }
}

/// 一条补丁的参数声明。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AskSpec {
    /// 参数名（模板里用 `{{key}}` 引用）。
    pub key: String,
    /// 提示语（弹窗 / 交互提示里显示）。
    #[serde(default)]
    pub label: Option<String>,
    /// 默认值（`--set` 没给时用这个）。
    #[serde(default)]
    pub default: Option<String>,
    /// 参数种类。
    #[serde(default)]
    pub kind: AskKind,
    /// 长度下限（**字符数**，不是字节数）。
    #[serde(default)]
    pub min: Option<usize>,
    /// 长度上限（**字符数**，不是字节数）。
    #[serde(default)]
    pub max: Option<usize>,
    /// 候选值（`kind: enum` 时必填）。
    #[serde(default)]
    pub choices: Option<Vec<String>>,
}

impl AskSpec {
    /// 显示用的名字。
    pub fn title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.key)
    }
}

/// 取值来源（写进报告，便于复现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// 来自 `--set key=value`。
    Cli,
    /// 来自 `--interactive` 的交互输入。
    Ask,
    /// 来自补丁表里的 `default`。
    Default,
    /// 来自表头 `meta`（比如 `meta.encoding`）。
    Meta,
    /// 来自补丁条目自己的 `encoding:` 字段。
    Spec,
}

impl Source {
    pub fn name(&self) -> &'static str {
        match self {
            Source::Cli => "cli",
            Source::Ask => "ask",
            Source::Default => "default",
            Source::Meta => "meta",
            Source::Spec => "spec",
        }
    }
}

/// 一个已经解析好的参数值。
#[derive(Debug, Clone, Serialize)]
pub struct ParamRecord {
    /// 参数名。
    pub key: String,
    /// 参数值（明文）。
    pub value: String,
    /// 来源：cli / ask / default / meta。
    pub source: String,
    /// 按落地编码转码后的字节数（能转码时才有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
}

/// 参数解析器：把 `--set` / `--interactive` / 表里的 default 汇成一张表。
#[derive(Debug, Clone)]
pub struct ParamResolver {
    /// 命令行给的参数（`--set key=value`）。
    sets: BTreeMap<String, String>,
    /// 是否交互式问用户。
    interactive: bool,
    /// 已经解析过的参数（按 key 缓存：同一个 key 只问一次）。
    cache: BTreeMap<String, ParamRecord>,
    /// 解析顺序（报告里按这个顺序列）。
    order: Vec<String>,
}

impl Default for ParamResolver {
    fn default() -> Self {
        Self::new(Vec::new(), false)
    }
}

impl ParamResolver {
    /// 新建一个解析器。`sets` 是 `--set key=value` 解析出来的键值对。
    pub fn new(sets: Vec<(String, String)>, interactive: bool) -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in sets {
            map.insert(k, v);
        }
        Self {
            sets: map,
            interactive,
            cache: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    /// 取一个已经解析好的参数。
    pub fn get(&self, key: &str) -> Option<&ParamRecord> {
        self.cache.get(key)
    }

    /// 命令行给了哪些参数（键 → 值）。
    pub fn cli_sets(&self) -> &BTreeMap<String, String> {
        &self.sets
    }

    /// 解析一条补丁的参数。同一个 key 只解析一次。
    pub fn resolve(&mut self, patch_id: &str, ask: &AskSpec) -> Result<ParamRecord> {
        if let Some(rec) = self.cache.get(&ask.key) {
            return Ok(rec.clone());
        }
        let (value, source) = if let Some(v) = self.sets.get(&ask.key) {
            (v.clone(), Source::Cli)
        } else if self.interactive {
            let input = prompt(patch_id, ask)?;
            if input.is_empty() {
                match &ask.default {
                    Some(d) => (d.clone(), Source::Default),
                    None => (input, Source::Ask),
                }
            } else {
                (input, Source::Ask)
            }
        } else if let Some(d) = &ask.default {
            (d.clone(), Source::Default)
        } else {
            bail!(
                "补丁 {} 需要参数 {}，但没有给值（用 --set {}=... 或 --interactive）",
                patch_id,
                ask.key,
                ask.key
            );
        };

        validate(patch_id, ask, &value)?;
        let rec = ParamRecord {
            key: ask.key.clone(),
            value,
            source: source.name().to_string(),
            bytes: None,
        };
        self.cache.insert(ask.key.clone(), rec.clone());
        self.order.push(ask.key.clone());
        Ok(rec)
    }

    /// 记下某个参数编码后的字节数（写进报告）。
    pub fn set_bytes(&mut self, key: &str, n: usize) {
        if let Some(rec) = self.cache.get_mut(key) {
            rec.bytes = Some(n);
        }
    }

    /// 所有解析过的参数（按解析顺序）。
    pub fn records(&self) -> Vec<ParamRecord> {
        self.order
            .iter()
            .filter_map(|k| self.cache.get(k).cloned())
            .collect()
    }
}

/// 参数值校验（字符数上下限 / 枚举候选 / 文件是否存在）。
fn validate(patch_id: &str, ask: &AskSpec, value: &str) -> Result<()> {
    let n = value.chars().count();
    if let Some(min) = ask.min {
        ensure!(
            n >= min,
            "补丁 {} 的参数 {} 太短（{} 字符，至少 {} 字符）",
            patch_id,
            ask.key,
            n,
            min
        );
    }
    if let Some(max) = ask.max {
        ensure!(
            n <= max,
            "补丁 {} 的参数 {} 太长（{} 字符，最多 {} 字符）",
            patch_id,
            ask.key,
            n,
            max
        );
    }
    match ask.kind {
        AskKind::Enum => {
            if let Some(choices) = &ask.choices {
                if !choices.is_empty() {
                    ensure!(
                        choices.iter().any(|c| c == value),
                        "补丁 {} 的参数 {} 的值「{}」不在候选列表里（{}）",
                        patch_id,
                        ask.key,
                        value,
                        choices.join(" / ")
                    );
                }
            }
        }
        AskKind::File => {
            ensure!(
                std::path::Path::new(value).exists(),
                "补丁 {} 的参数 {} 指向的文件不存在：{}",
                patch_id,
                ask.key,
                value
            );
        }
        AskKind::Path => {
            ensure!(
                !value.trim().is_empty(),
                "补丁 {} 的参数 {}（路径）不能为空",
                patch_id,
                ask.key
            );
        }
        AskKind::Text => {}
    }
    Ok(())
}

/// 交互式问用户要一个值。
fn prompt(patch_id: &str, ask: &AskSpec) -> Result<String> {
    let mut out = std::io::stdout();
    writeln!(out)?;
    writeln!(out, "┌ 参数（补丁 {patch_id}）")?;
    writeln!(out, "│ 说明：{}", ask.title())?;
    if let Some(d) = &ask.default {
        writeln!(out, "│ 默认：{d}")?;
    }
    writeln!(out, "│ 类型：{}", ask.kind.name())?;
    if let AskKind::Enum = ask.kind {
        if let Some(c) = &ask.choices {
            writeln!(out, "│ 可选：{}", c.join(" / "))?;
        }
    }
    if ask.min.is_some() || ask.max.is_some() {
        writeln!(
            out,
            "│ 长度：{} ~ {} 字符",
            ask.min.unwrap_or(0),
            ask.max.unwrap_or(usize::MAX)
        )?;
    }
    write!(out, "└ 请输入 > ")?;
    out.flush()?;

    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line)?;
    if n == 0 {
        // stdin 到头了
        writeln!(std::io::stdout())?;
        bail!("补丁 {} 的参数 {} 没有输入（stdin 已结束）", patch_id, ask.key);
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

// ---------------------------------------------------------------- 模板

/// 模板的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// 普通文本。
    Literal(String),
    /// `{{key}}` 占位符。
    Placeholder(String),
}

/// 把模板切成「普通文本 + 占位符」。
pub fn split_template(s: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let key = after[..end].trim().to_string();
        if key.is_empty() {
            break;
        }
        if start > 0 {
            parts.push(Part::Literal(rest[..start].to_string()));
        }
        parts.push(Part::Placeholder(key));
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        parts.push(Part::Literal(rest.to_string()));
    }
    parts
}

/// 模板里有没有占位符。
pub fn has_placeholder(s: &str) -> bool {
    s.contains("{{") && s.contains("}}")
}

/// 文本模板：把占位符替换成参数值（**明文**，不转码）。
pub fn render_text(template: &str, params: &BTreeMap<String, String>, who: &str) -> Result<String> {
    let mut out = String::new();
    for part in split_template(template) {
        match part {
            Part::Literal(s) => out.push_str(&s),
            Part::Placeholder(key) => {
                let v = params.get(&key).ok_or_else(|| {
                    anyhow::anyhow!("{who} 的模板里 {{{{{key}}}}} 没有对应的参数值")
                })?;
                out.push_str(v);
            }
        }
    }
    Ok(out)
}

/// hex 模板：普通段按 hex 解析，占位符按落地编码转码后拼进去。
///
/// 也就是说 `replace: "52 4F 54 57 00 {{title}}"` =
/// 前 4 个 hex 字节 + 标题的编码字节。
pub fn render_hex(
    template: &str,
    params: &BTreeMap<String, String>,
    enc: Encoding,
    who: &str,
) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    for part in split_template(template) {
        match part {
            Part::Literal(s) => {
                if !s.trim().is_empty() {
                    out.extend(hex::parse_bytes(&s).with_context(|| {
                        anyhow::anyhow!("{who} 的模板里有一段不是合法 hex：{}", s.trim())
                    })?);
                }
            }
            Part::Placeholder(key) => {
                let v = params.get(&key).ok_or_else(|| {
                    anyhow::anyhow!("{who} 的模板里 {{{{{key}}}}} 没有对应的参数值")
                })?;
                out.extend(encode::encode_for(v, enc, &key)?);
            }
        }
    }
    Ok(out)
}

/// 参数值表（key → 明文值）。
pub type ParamValues = BTreeMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 切模板() {
        let p = split_template("AA {{x}} BB");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Part::Literal("AA ".into()));
        assert_eq!(p[1], Part::Placeholder("x".into()));
        assert_eq!(p[2], Part::Literal(" BB".into()));
    }

    #[test]
    fn 切模板_没有占位符() {
        let p = split_template("52 61 67");
        assert_eq!(p.len(), 1);
        assert!(matches!(&p[0], Part::Literal(s) if s == "52 61 67"));
        assert!(!has_placeholder("52 61 67"));
        assert!(has_placeholder("{{a}}"));
    }

    #[test]
    fn 渲染文本模板() {
        let mut m = ParamValues::new();
        m.insert("t".into(), "示例".into());
        let s = render_text("标题：{{t}}！", &m, "测试").unwrap();
        assert_eq!(s, "标题：示例！");
    }

    #[test]
    fn 渲染hex模板() {
        let mut m = ParamValues::new();
        m.insert("t".into(), "RO".into());
        let b = render_hex("52 4F {{t}}", &m, Encoding::Gbk, "测试").unwrap();
        assert_eq!(b, vec![0x52, 0x4F, 0x52, 0x4F]);
    }

    #[test]
    fn 命令行优先于默认值() {
        let ask = AskSpec {
            key: "title".into(),
            label: None,
            default: Some("Ragnarok".into()),
            kind: AskKind::Text,
            min: None,
            max: None,
            choices: None,
        };
        let mut r = ParamResolver::new(vec![("title".into(), "示例".into())], false);
        let rec = r.resolve("x", &ask).unwrap();
        assert_eq!(rec.value, "示例");
        assert_eq!(rec.source, "cli");
    }

    #[test]
    fn 没有值也没有默认值就报错() {
        let ask = AskSpec {
            key: "title".into(),
            label: None,
            default: None,
            kind: AskKind::Text,
            min: None,
            max: None,
            choices: None,
        };
        let mut r = ParamResolver::new(vec![], false);
        assert!(r.resolve("x", &ask).is_err());
    }

    #[test]
    fn 超出字符上限就报错() {
        let ask = AskSpec {
            key: "title".into(),
            label: None,
            default: Some("Ragnarok".into()),
            kind: AskKind::Text,
            min: Some(1),
            max: Some(3),
            choices: None,
        };
        let mut r = ParamResolver::new(vec![], false);
        assert!(r.resolve("x", &ask).is_err());
    }

    #[test]
    fn 枚举值不在候选里就报错() {
        let ask = AskSpec {
            key: "encoding".into(),
            label: None,
            default: Some("gbk".into()),
            kind: AskKind::Enum,
            min: None,
            max: None,
            choices: Some(vec!["gbk".into(), "utf8".into()]),
        };
        let mut r = ParamResolver::new(vec![("encoding".into(), "ebcdic".into())], false);
        assert!(r.resolve("x", &ask).is_err());
    }
}
