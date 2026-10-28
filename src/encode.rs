//! 文本编码转换：GBK / CP949(EUC-KR) / UTF-8 / Shift_JIS / CP874。
//!
//! 客户端 langtype=1 走系统 ANSI = GBK，所以中文补丁用 GBK；
//! 韩文原文是 CP949（EUC-KR 的 Windows 扩展）。
//!
//! **YAML 一律 UTF-8**，写入 exe 的字节才按这里的编码转：
//! 客户端按系统 ANSI 代码页解码 exe 里的字符串，写错编码就是整片乱码。

use anyhow::{bail, Result};

/// 支持的编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Gbk,
    Cp949,
    Utf8,
    /// Shift_JIS（日文客户端）。
    Sjis,
    /// CP874（泰文）。
    Cp874,
}

impl Encoding {
    /// 从名字解析（不区分大小写）。
    pub fn from_name(name: &str) -> Result<Encoding> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gbk" | "gb2312" | "gb18030" | "cp936" | "ansi" => Ok(Encoding::Gbk),
            "cp949" | "euc-kr" | "euckr" | "ks_c_5601" | "korean" => Ok(Encoding::Cp949),
            "utf8" | "utf-8" => Ok(Encoding::Utf8),
            "sjis" | "shift_jis" | "shift-jis" | "cp932" | "ms932" => {
                Ok(Encoding::Sjis)
            }
            "cp874" | "windows-874" | "tis-620" | "iso-8859-11" => Ok(Encoding::Cp874),
            other => bail!("未知编码：{other}（支持 gbk / cp949 / utf8 / sjis / cp874）"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Encoding::Gbk => "gbk",
            Encoding::Cp949 => "cp949",
            Encoding::Utf8 => "utf8",
            Encoding::Sjis => "sjis",
            Encoding::Cp874 => "cp874",
        }
    }

    /// 报错信息里用的名字（大写，和命令行参数名不完全一样）。
    pub fn display_name(&self) -> &'static str {
        match self {
            Encoding::Gbk => "GBK",
            Encoding::Cp949 => "CP949",
            Encoding::Utf8 => "UTF-8",
            Encoding::Sjis => "Shift_JIS",
            Encoding::Cp874 => "CP874",
        }
    }

    /// 把单个字符编成字节（用来找「哪个字符编不出来」）。
    fn encode_char(ch: char, enc: Encoding) -> (Vec<u8>, bool) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let (bytes, _used, had_errors) = match enc {
            Encoding::Gbk => encoding_rs::GBK.encode(s),
            Encoding::Cp949 => encoding_rs::EUC_KR.encode(s),
            Encoding::Utf8 => encoding_rs::UTF_8.encode(s),
            Encoding::Sjis => encoding_rs::SHIFT_JIS.encode(s),
            Encoding::Cp874 => encoding_rs::WINDOWS_874.encode(s),
        };
        (bytes.to_vec(), had_errors)
    }
}

/// 文本里第一个编不出来的字符（没有则返回 None）。
pub fn first_bad_char(text: &str, enc: Encoding) -> Option<char> {
    for ch in text.chars() {
        let (_, bad) = Encoding::encode_char(ch, enc);
        if bad {
            return Some(ch);
        }
    }
    None
}

/// 把文本编码成字节。编码不出来的字符会报错（不静默变成 `?`）。
pub fn encode(text: &str, enc: Encoding) -> Result<Vec<u8>> {
    let (bytes, _used, had_errors) = match enc {
        Encoding::Gbk => encoding_rs::GBK.encode(text),
        Encoding::Cp949 => encoding_rs::EUC_KR.encode(text),
        Encoding::Utf8 => encoding_rs::UTF_8.encode(text),
        Encoding::Sjis => encoding_rs::SHIFT_JIS.encode(text),
        Encoding::Cp874 => encoding_rs::WINDOWS_874.encode(text),
    };
    if had_errors {
        let bad = first_bad_char(text, enc)
            .map(|c| c.to_string())
            .unwrap_or_else(|| text.to_string());
        bail!("文本里的「{bad}」无法用 {} 表示", enc.display_name());
    }
    Ok(bytes.to_vec())
}

/// 把文本编码成字节，失败时的报错格式固定（启动器好展示）：
///
/// ```text
/// 参数 {who} 的文本「{ch}」无法用 GBK 表示（请改用 --encoding utf8 或换同义字）
/// ```
///
/// `who` 是参数名（`ask.key`）或补丁 id。
pub fn encode_for(text: &str, enc: Encoding, who: &str) -> Result<Vec<u8>> {
    match encode(text, enc) {
        Ok(b) => Ok(b),
        Err(_) => {
            let ch = first_bad_char(text, enc)
                .map(|c| c.to_string())
                .unwrap_or_else(|| text.to_string());
            bail!(
                "参数 {who} 的文本「{ch}」无法用 {} 表示（请改用 --encoding utf8 或换同义字）",
                enc.display_name()
            );
        }
    }
}

/// 解码（调试用）。
#[allow(dead_code)]
pub fn decode(bytes: &[u8], enc: Encoding) -> Result<String> {
    let (cow, _used, had_errors) = match enc {
        Encoding::Gbk => encoding_rs::GBK.decode(bytes),
        Encoding::Cp949 => encoding_rs::EUC_KR.decode(bytes),
        Encoding::Utf8 => encoding_rs::UTF_8.decode(bytes),
        Encoding::Sjis => encoding_rs::SHIFT_JIS.decode(bytes),
        Encoding::Cp874 => encoding_rs::WINDOWS_874.decode(bytes),
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
        assert_eq!(Encoding::from_name("utf8").unwrap(), Encoding::Utf8);
        assert_eq!(Encoding::from_name("sjis").unwrap(), Encoding::Sjis);
        assert_eq!(Encoding::from_name("cp874").unwrap(), Encoding::Cp874);
        assert!(Encoding::from_name("ebcdic").is_err());
    }

    #[test]
    fn 找出编不出来的字符() {
        assert_eq!(first_bad_char("消息", Encoding::Gbk), None);
        assert_eq!(first_bad_char("a😀b", Encoding::Gbk), Some('😀'));
        assert_eq!(first_bad_char("😀", Encoding::Utf8), None);
    }

    #[test]
    fn 报错格式固定() {
        let e = encode_for("😀", Encoding::Gbk, "title").unwrap_err().to_string();
        assert_eq!(
            e,
            "参数 title 的文本「😀」无法用 GBK 表示（请改用 --encoding utf8 或换同义字）"
        );
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
