//! 十六进制模式解析 / 格式化。
//!
//! 支持三种写法（可以混用）：
//!
//! * `"B8 DE BD C3"`  —— 空格分隔
//! * `"B8DEBDC3"`     —— 连写
//! * `"B8 ?? C3"`     —— `??` 是通配（匹配任意字节）
//! * `"B? C3"`       —— 半字节通配（`B?` = 高 nibble 固定、低 nibble 任意）

use anyhow::{bail, Result};

/// 模式里的一个字节。
///
/// `mask` 里的位才是「必须相等」的位：`FF` = 完全匹配，
/// `F0` = 只有高 nibble 匹配（`B?`），`00` = 任意（`??`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatByte {
    pub value: u8,
    pub mask: u8,
}

impl PatByte {
    /// 这个模式字节是否匹配 `b`。
    pub fn matches(&self, b: u8) -> bool {
        (b & self.mask) == (self.value & self.mask)
    }

    /// 是不是通配字节（任意值都能匹配）。
    pub fn is_wildcard(&self) -> bool {
        self.mask != 0xFF
    }
}

/// 解析搜索模式（允许通配）。
pub fn parse_pattern(s: &str) -> Result<Vec<PatByte>> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.is_empty() {
            continue;
        }
        if tok.len() % 2 != 0 {
            bail!("非法十六进制片段（长度必须是偶数）：{tok}");
        }
        for i in (0..tok.len()).step_by(2) {
            out.push(parse_byte(&tok[i..i + 2])?);
        }
    }
    if out.is_empty() {
        bail!("空的搜索模式");
    }
    Ok(out)
}

/// 解析精确字节串（不允许通配），用来读 `replace`。
pub fn parse_bytes(s: &str) -> Result<Vec<u8>> {
    let pat = parse_pattern(s)?;
    if pat.iter().any(|p| p.is_wildcard()) {
        bail!("字节串里不允许通配符");
    }
    Ok(pat.iter().map(|p| p.value).collect())
}

/// 解析「替换字节串」：允许通配。
///
/// 和 [`parse_bytes`] 的区别：`None` 表示「这一位保留原字节」。
/// 这样 `find: "80 3C ?? 00 7C ?? 40"` 配
/// `replace: "80 3C ?? 00 90 90 40"` 就能只改想改的那几个字节。
pub fn parse_replace(s: &str) -> Result<Vec<Option<u8>>> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.is_empty() {
            continue;
        }
        if tok.len() % 2 != 0 {
            bail!("非法十六进制片段（长度必须是偶数）：{tok}");
        }
        for i in (0..tok.len()).step_by(2) {
            let b = parse_byte(&tok[i..i + 2])?;
            out.push(if b.is_wildcard() { None } else { Some(b.value) });
        }
    }
    if out.is_empty() {
        bail!("空的替换字节串");
    }
    Ok(out)
}

/// 把「替换字节串」里的通配位填成原字节，得到实际要写的字节。
pub fn resolve_replace(spec: &[Option<u8>], original: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(spec.len());
    for (i, s) in spec.iter().enumerate() {
        match s {
            Some(v) => out.push(*v),
            None => {
                let o = original
                    .get(i)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("替换字节串比原串长，没法保留第 {i} 位的原字节"));
                out.push(o?);
            }
        }
    }
    Ok(out)
}

fn parse_byte(tok: &str) -> Result<PatByte> {
    let b = tok.as_bytes();
    if b.len() != 2 {
        bail!("非法十六进制片段：{tok}");
    }
    let hi = nibble(b[0])?;
    let lo = nibble(b[1])?;
    Ok(match (hi, lo) {
        (Some(h), Some(l)) => PatByte {
            value: (h << 4) | l,
            mask: 0xFF,
        },
        (Some(h), None) => PatByte {
            value: h << 4,
            mask: 0xF0,
        },
        (None, Some(l)) => PatByte {
            value: l,
            mask: 0x0F,
        },
        (None, None) => PatByte {
            value: 0,
            mask: 0x00,
        },
    })
}

fn nibble(c: u8) -> Result<Option<u8>> {
    match c {
        b'0'..=b'9' => Ok(Some(c - b'0')),
        b'a'..=b'f' => Ok(Some(c - b'a' + 10)),
        b'A'..=b'F' => Ok(Some(c - b'A' + 10)),
        b'?' => Ok(None),
        _ => bail!("非法十六进制字符：{}", c as char),
    }
}

/// 把字节串格式化成 `"CF FB CF A2"`。
pub fn format_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析普通十六进制() {
        let p = parse_pattern("B8 DE BD C3").unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].value, 0xB8);
        assert!(!p[0].is_wildcard());
    }

    #[test]
    fn 解析连写十六进制() {
        let p = parse_pattern("B8DEBDC3").unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p[3].value, 0xC3);
    }

    #[test]
    fn 解析通配() {
        let p = parse_pattern("B8 ?? C3").unwrap();
        assert_eq!(p.len(), 3);
        assert!(!p[0].is_wildcard());
        assert!(p[1].is_wildcard());
        assert!(p[1].matches(0x00));
        assert!(p[1].matches(0xFF));
        assert!(p[2].value == 0xC3);
    }

    #[test]
    fn 解析半字节通配() {
        let p = parse_pattern("B?").unwrap();
        assert!(p[0].is_wildcard());
        assert!(p[0].matches(0xB0));
        assert!(p[0].matches(0xBF));
        assert!(!p[0].matches(0xC0));
    }

    #[test]
    fn 奇数长度报错() {
        assert!(parse_pattern("B8 D").is_err());
        assert!(parse_pattern("ZZ").is_err());
    }

    #[test]
    fn 字节串不允许通配() {
        assert!(parse_bytes("B8 ?? C3").is_err());
        assert_eq!(parse_bytes("CF FB").unwrap(), vec![0xCF, 0xFB]);
    }

    #[test]
    fn 格式化() {
        assert_eq!(format_bytes(&[0xCF, 0xFB, 0xCF, 0xA2]), "CF FB CF A2");
    }
}
