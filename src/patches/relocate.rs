//! 「搬字符串」机制的原语。
//!
//! WARP 的 `CustomPath.qjs` 干的事不是 find/replace，而是**改指令**：
//!
//! ```text
//! 1. 在 exe 里找到「原路径字符串」（.rdata 里的一个 C 字符串）
//! 2. 找到引用它的指令（`push <字符串地址>`）
//! 3. 把新路径写到一个可写的地方（这里是 .text 段尾的文件空洞）
//! 4. 把那条 push 的立即数改成新字符串的地址
//! ```
//!
//! 本模块只提供「找」的能力（字符串、引用点、上次搬的位置），
//! 写入由各个补丁处理器（[`super::custom_lub`]、[`super::window_title`]）完成。
//!
//! ## 搬进空洞的字符串长什么样
//!
//! ```text
//! +----------------+------------------+
//! | 标记 12 字节   | 新字符串（含 \0）|
//! +----------------+------------------+
//! ```
//!
//! 标记 = 魔数 `WRSSTR\x01`（8 字节）+ **原字符串的 VA**（4 字节，小端）。
//! 带上原 VA 是为了：同一个补丁第二次运行时能**认出自己上次搬的地方**
//! （幂等判定、以及换路径重打时能找回引用点），别的补丁不会认错。

use std::collections::BTreeSet;

use anyhow::Result;

use crate::engine::Engine;
use crate::hex::PatByte;

/// 搬到空洞里的字符串前面写的魔数。
pub const CAVE_MAGIC: &[u8; 8] = b"WRSSTR01";
/// 标记总长：魔数 8 字节 + 原字符串 VA 4 字节。
pub const CAVE_MARK_LEN: usize = 12;
/// 路径字符串的最大长度（**含结束符**，单位：字节）。
pub const MAX_PATH_BYTES: usize = 64;

/// 一个在文件里找到的 C 字符串。
#[derive(Debug, Clone)]
pub struct FoundString {
    /// 第一个字节的文件偏移。
    pub offset: usize,
    /// 虚拟地址。
    pub va: u32,
    /// 字符串内容（不含结束符）。
    pub text: String,
}

/// 引用一个地址的指令形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `push imm32`（opcode 0x68）。
    Push,
    /// `mov r32, imm32`（opcode 0xB8..0xBF）。
    MovReg,
    /// `mov dword ptr [disp], imm32`（opcode 0xC7，ModRM 05 / 45 / 85）。
    MovMem,
}

impl RefKind {
    /// 中文名（报告里用）。
    pub fn name(&self) -> &'static str {
        match self {
            RefKind::Push => "push imm32",
            RefKind::MovReg => "mov r32, imm32",
            RefKind::MovMem => "mov [mem], imm32",
        }
    }
}

/// 一处「把某个 VA 当 32 位立即数用」的指令。
#[derive(Debug, Clone, Copy)]
pub struct ImmRef {
    /// 指令第一条字节的文件偏移。
    pub op_offset: usize,
    /// 32 位立即数的文件偏移（= op_offset + 指令里立即数的位置）。
    pub imm_offset: usize,
    /// 引用形式。
    pub kind: RefKind,
    /// 整条指令的长度（字节）。
    pub insn_len: usize,
}

/// 通配匹配：`*` 匹配任意长度（可以为空），**大小写不敏感**。
///
/// 用来匹配「原路径」特征串，例如 `system\OngoingQuestInfoList*`
/// 能同时命中 `..._Sakray` 和 `..._True` 两个字符串。
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (n, m) = (p.len(), t.len());
    let mut dp = vec![vec![false; m + 1]; n + 1];
    dp[0][0] = true;
    for i in 1..=n {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if p[i - 1] == '*' {
                dp[i - 1][j] || dp[i][j - 1]
            } else {
                p[i - 1] == t[j - 1] && dp[i - 1][j - 1]
            };
        }
    }
    dp[n][m]
}

/// 模式里最长的那段字面量（不含 `*`），当搜索锚点用。
fn longest_literal(pattern: &str) -> String {
    pattern
        .split('*')
        .filter(|s| !s.is_empty())
        .max_by_key(|s| s.len())
        .unwrap_or("")
        .to_string()
}

/// 大小写不敏感地找一段 ASCII 字节串。
fn find_ci(data: &[u8], needle: &[u8], from: usize, to: usize) -> Option<usize> {
    if needle.is_empty() {
        return if from < to { Some(from) } else { None };
    }
    if needle.len() > to.saturating_sub(from) {
        return None;
    }
    if !needle.iter().all(|b| b.is_ascii()) {
        // 非 ASCII 的锚点不转小写，直接按原样找
        let end = to - needle.len();
        let mut i = from;
        while i <= end {
            if &data[i..i + needle.len()] == needle {
                return Some(i);
            }
            i += 1;
        }
        return None;
    }
    let end = to - needle.len();
    let mut i = from;
    while i <= end {
        let mut ok = true;
        for (k, b) in needle.iter().enumerate() {
            if data[i + k].to_ascii_lowercase() != b.to_ascii_lowercase() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 找出包含 `p` 的那个 C 字符串的起止（不含结束符）。
///
/// 只认「可打印 ASCII + NUL 结尾」的独立字符串；二进制数据里的巧合命中会被丢掉。
fn string_bounds(data: &[u8], p: usize) -> Option<(usize, usize)> {
    const MAX_LEN: usize = 512;
    let mut s = p;
    while s > 0 {
        let b = data[s - 1];
        if b == 0 {
            break;
        }
        if !(0x20..=0x7e).contains(&b) {
            return None;
        }
        s -= 1;
        if p - s > MAX_LEN {
            return None;
        }
    }
    let mut e = p;
    while e < data.len() && data[e] != 0 {
        if !(0x20..=0x7e).contains(&data[e]) {
            return None;
        }
        e += 1;
        if e - s > MAX_LEN {
            return None;
        }
    }
    if e >= data.len() {
        // 没找到结束符（文件被截断了）
        return None;
    }
    Some((s, e))
}

/// 在范围里找所有匹配 `pattern` 的 C 字符串。
///
/// * `pattern` 里 `*` 是通配，匹配大小写不敏感。
/// * 返回的每个字符串都是**独立**的 NUL 结尾串（前后有 NUL 边界）。
pub fn find_strings(eng: &Engine, pattern: &str, range: (usize, usize)) -> Vec<FoundString> {
    let (start, end) = range;
    let end = end.min(eng.data.len());
    if start >= end {
        return Vec::new();
    }
    let anchor = longest_literal(pattern);
    let anchor_bytes = anchor.as_bytes();
    let mut out: Vec<FoundString> = Vec::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut i = start;
    while i < end {
        let Some(p) = find_ci(&eng.data, anchor_bytes, i, end) else {
            break;
        };
        i = p + 1;
        let Some((s, e)) = string_bounds(&eng.data, p) else {
            continue;
        };
        if !seen.insert(s) {
            continue;
        }
        let text = String::from_utf8_lossy(&eng.data[s..e]).into_owned();
        if wildcard_match(pattern, &text) {
            out.push(FoundString {
                offset: s,
                va: eng.va_of_offset(s).unwrap_or(0),
                text,
            });
        }
    }
    out
}

/// 读某个偏移处的 NUL 结尾字符串（不含结束符）。
pub fn read_cstr(data: &[u8], off: usize) -> Option<&[u8]> {
    if off >= data.len() {
        return None;
    }
    let end = data[off..].iter().position(|b| *b == 0)? + off;
    Some(&data[off..end])
}

/// 找出 `.text` 里所有 `push <va>` 的落点（返回 opcode 0x68 的文件偏移）。
pub fn find_push_sites(eng: &Engine, va: u32, range: (usize, usize)) -> Vec<usize> {
    let mut pat: Vec<PatByte> = Vec::with_capacity(5);
    pat.push(PatByte {
        value: 0x68,
        mask: 0xFF,
    });
    for b in va.to_le_bytes() {
        pat.push(PatByte {
            value: b,
            mask: 0xFF,
        });
    }
    eng.find_pattern(&pat, range)
}

/// 找出所有「把这个 VA 当 32 位立即数用」的指令。
///
/// 只认 `kinds` 里列出的形式，避免把数据里的巧合字节当成指令。
pub fn find_imm_refs(
    eng: &Engine,
    va: u32,
    kinds: &[RefKind],
    range: (usize, usize),
) -> Vec<ImmRef> {
    let hits = eng.find_text(&va.to_le_bytes(), range);
    let mut out: Vec<ImmRef> = Vec::new();
    for h in hits {
        if h == 0 {
            continue;
        }
        // 注意：`mov [disp32], imm32` 的立即数前面是 disp32 的一部分，
        // 所以 opcode 不一定紧挨着立即数 —— 每种形式各自判断。
        let prev = eng.data[h - 1];
        if prev == 0x68 && kinds.contains(&RefKind::Push) {
            out.push(ImmRef {
                op_offset: h - 1,
                imm_offset: h,
                kind: RefKind::Push,
                insn_len: 5,
            });
            continue;
        }
        if (0xb8..=0xbf).contains(&prev) && kinds.contains(&RefKind::MovReg) {
            out.push(ImmRef {
                op_offset: h - 1,
                imm_offset: h,
                kind: RefKind::MovReg,
                insn_len: 5,
            });
            continue;
        }
        if kinds.contains(&RefKind::MovMem) {
            // C7 05 disp32 imm32 / C7 85 disp32 imm32：opcode 在 h-6，ModRM 在 h-5
            let f1 = h >= 6
                && eng.data[h - 6] == 0xc7
                && (eng.data[h - 5] == 0x05 || eng.data[h - 5] == 0x85);
            // C7 45 disp8 imm32：opcode 在 h-3，ModRM 在 h-2
            let f2 = h >= 3 && eng.data[h - 3] == 0xc7 && eng.data[h - 2] == 0x45;
            if f1 || f2 {
                out.push(ImmRef {
                    op_offset: h - 6.min(h - 3),
                    imm_offset: h,
                    kind: RefKind::MovMem,
                    insn_len: if f1 { 10 } else { 7 },
                });
            }
        }
    }
    out
}

/// 搬到空洞里的字符串前面的标记（魔数 + 原字符串 VA）。
pub fn cave_mark_bytes(old_va: u32) -> [u8; CAVE_MARK_LEN] {
    let mut b = [0u8; CAVE_MARK_LEN];
    b[..CAVE_MAGIC.len()].copy_from_slice(CAVE_MAGIC);
    b[CAVE_MAGIC.len()..].copy_from_slice(&old_va.to_le_bytes());
    b
}

/// 找出「上次搬的时候写在空洞里的标记」的文件偏移（可能不止一个：
/// 用户换过路径就会留下旧的落点）。
pub fn find_cave_marks(eng: &Engine, old_va: u32) -> Vec<usize> {
    let needle = cave_mark_bytes(old_va);
    eng.find_text(&needle, (0, eng.data.len()))
}

/// 空洞里的字符串 VA（标记在字符串前面）。
pub fn cave_string_va(mark_va: u32) -> u32 {
    mark_va + CAVE_MARK_LEN as u32
}

/// 工具：把一段字节写成 hex（报告里用）。
pub fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// 工具：读出某个 VA 处的 C 字符串（解出 imm32 指向的内容，verify 用）。
pub fn string_at_va(eng: &Engine, va: u32) -> Result<Option<Vec<u8>>> {
    let Ok(off) = eng.offset_of_va(va) else {
        return Ok(None);
    };
    Ok(read_cstr(&eng.data, off).map(|s| s.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 通配匹配_星号匹配任意长度() {
        assert!(wildcard_match("a*b", "ab"));
        assert!(wildcard_match("a*b", "axxb"));
        assert!(wildcard_match("*b", "aaab"));
        assert!(wildcard_match("a*", "aaaa"));
        assert!(wildcard_match("*", "anything"));
        assert!(!wildcard_match("a*b", "abx"));
        assert!(!wildcard_match("abc", "abx"));
    }

    #[test]
    fn 通配匹配_大小写不敏感() {
        assert!(wildcard_match(
            "system\\OngoingQuestInfoList*",
            "System\\OngoingQuestInfoList_Sakray"
        ));
        assert!(wildcard_match("SYSTEM\\TIPBOX.LUB", "system\\tipbox.lub"));
    }

    #[test]
    fn 标记_含原字符串的_va() {
        let b = cave_mark_bytes(0x00FC784C);
        assert_eq!(&b[..8], CAVE_MAGIC);
        assert_eq!(&b[8..], &0x00FC784Cu32.to_le_bytes());
        assert_eq!(b.len(), CAVE_MARK_LEN);
    }

    #[test]
    fn 读_c_字符串() {
        let data = b"abc\0def\0";
        let abc: &[u8] = b"abc";
        let def: &[u8] = b"def";
        let empty: &[u8] = b"";
        assert_eq!(read_cstr(&data[..], 0), Some(abc));
        assert_eq!(read_cstr(&data[..], 4), Some(def));
        assert_eq!(read_cstr(&data[..], 3), Some(empty));
    }

    #[test]
    fn 最长字面量做锚点() {
        assert_eq!(longest_literal("a*bb*ccc"), "ccc");
        assert_eq!(longest_literal("abc"), "abc");
        assert_eq!(longest_literal("**"), "");
    }
}
