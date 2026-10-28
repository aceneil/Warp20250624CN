//! 文本编码转换：GBK / CP949(EUC-KR) / UTF-8。
//!
//! 客户端 langtype=1 走系统 ANSI = GBK，所以中文补丁用 GBK；
//! 韩文原文是 CP949（EUC-KR 的 Windows 扩展）。

use anyhow::{bail, Result};

/// 支持的编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Gbk,
    Cp949,
    Utf8,
}

impl Encoding {
    /// 从名字解析（不区分大小写），`None` = UTF-8。
    pub fn from_name(name: &str) -> Result<Encoding> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gbk" | "gb2312" | "gb18030" | "cp936" | "ansi" => Ok(Encoding::Gbk),
            "cp949" | "euc-kr" | "euckr" | "ks_c_5601" | "korean" => Ok(Encoding::Cp949),
            "utf8" | "utf-8" => Ok(Encoding::Utf8),
            other => bail!("未知编码：{other}（支持 gbk / cp949 / utf8）"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Encoding::Gbk => "gbk",
            Encoding::Cp949 => "cp949",
            Encoding::Utf8 => "utf8",
        }
    }
}

/// 把文本编码成字节。编码不出来的字符会报错（不静默变成 `?`）。
pub fn encode(text: &str, enc: Encoding) -> Result<Vec<u8>> {
    let (bytes, _used, had_errors) = match enc {
        Encoding::Gbk => encoding_rs::GBK.encode(text),
        Encoding::Cp949 => encoding_rs::EUC_KR.encode(text),
        Encoding::Utf8 => encoding_rs::UTF_8.encode(text),
    };
    if had_errors {
        bail!("文本里有字符无法用 {} 编码：{text}", enc.name());
    }
    Ok(bytes.to_vec())
}

/// 解码（调试用）。
#[allow(dead_code)]
pub fn decode(bytes: &[u8], enc: Encoding) -> Result<String> {
    let (cow, _used, had_errors) = match enc {
        Encoding::Gbk => encoding_rs::GBK.decode(bytes),
        Encoding::Cp949 => encoding_rs::EUC_KR.decode(bytes),
        Encoding::Utf8 => encoding_rs::UTF_8.decode(bytes),
    };
    if had_errors {
        bail!("字节串不是合法的 {}", enc.name());
    }
    Ok(cow.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbk_编码中文() {
        assert_eq!(encode("消息", Encoding::Gbk).unwrap(), vec![0xCF, 0xFB, 0xCF, 0xA2]);
    }

    #[test]
    fn gbk_编码_ascii() {
        assert_eq!(encode("Total : %d 点", Encoding::Gbk).unwrap().len(), 13);
    }

    #[test]
    fn cp949_编码韩文() {
        // "메시지" 的 CP949 字节
        assert_eq!(
            encode("메시지", Encoding::Cp949).unwrap(),
            vec![0xB8, 0xDE, 0xBD, 0xC3, 0xC1, 0xF6]
        );
    }

    #[test]
    fn utf8_编码() {
        assert_eq!(encode("消息", Encoding::Utf8).unwrap(), vec![0xE6, 0xB6, 0x88, 0xE6, 0x81, 0xAF]);
    }

    #[test]
    fn 编码名解析() {
        assert_eq!(Encoding::from_name("GBK").unwrap(), Encoding::Gbk);
        assert_eq!(Encoding::from_name("cp949").unwrap(), Encoding::Cp949);
        assert!(Encoding::from_name("ebcdic").is_err());
    }

    #[test]
    fn gbk_无法编码时报错() {
        // emoji 不在 GBK / GB18030 里
        assert!(encode("😀", Encoding::Gbk).is_err());
    }

    #[test]
    fn 解码回环() {
        let b = encode("消息", Encoding::Gbk).unwrap();
        assert_eq!(decode(&b, Encoding::Gbk).unwrap(), "消息");
    }
}
