//! 补丁引擎：WARP 里那些「搜特征串 → 原地覆写」的原语。
//!
//! 覆盖 WARP 208 个 .qjs 里最常用的能力：
//!
//! | 原语 | 本模块对应 |
//! |---|---|
//! | `FindHex` | [`Engine::find_hex`] |
//! | `FindHexN` | [`Engine::find_hex_nth`] |
//! | `FindLastHex` | [`Engine::find_last_hex`] |
//! | `FindText` | [`Engine::find_text`] |
//! | `GetU8/U16/U32` | [`Engine::read_u8_va`] / `read_u16_va` / `read_u32_va` |
//! | `SetU8/U16/U32` | [`Engine::write_u8_va`] / `write_u16_va` / `write_u32_va` |
//! | `SetHex` | [`Engine::write_hex_va`] |
//! | `SetNOPs` | [`Engine::write_nops_va`] |
//! | `SetJMP` | [`Engine::write_jmp_va`] |
//! | `GetTgtAddr` | [`Engine::get_tgt_addr`] |
//!
//! **所有写操作都会先做边界检查**：不能写到段外、文件外，也不能跨段。
//!
//! ## 地址约定
//!
//! 本模块的「VA」是**完整虚拟地址**（镜像基址 + RVA），和 WARP 脚本里的
//! 地址一致（客户端是 PE32，基址 0x00400000，所以 .text 开头是 0x00401000）。
//! 如果传入的值小于镜像基址，会当成 RVA 处理。

use anyhow::{bail, ensure, Context, Result};

use crate::hex::{self, PatByte};
use crate::pe::Pe;

/// 补丁引擎：持有 exe 的字节缓冲 + PE 解析结果。
pub struct Engine {
    pub data: Vec<u8>,
    pub pe: Pe,
}

/// 一次跳转写入的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum JmpKind {
    /// `E9 rel32`（5 字节）
    Rel32,
    /// `EB rel8`（2 字节，5 字节放不下时用）
    Rel8,
}

impl JmpKind {
    pub fn len(&self) -> usize {
        match self {
            JmpKind::Rel32 => 5,
            JmpKind::Rel8 => 2,
        }
    }
}

impl Engine {
    /// 从原始字节构造引擎（会做一次 PE 解析）。
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let (pe, _notes) = crate::pe::analyze(&data)?;
        Ok(Engine { data, pe })
    }

    // ---------------------------------------------------------------- 搜索

    /// 搜索范围：给了段名就用那个段的 raw 范围，否则整文件。
    pub fn range_of(&self, section: Option<&str>) -> Result<(usize, usize)> {
        match section {
            Some(name) => self
                .pe
                .section_range(name)
                .with_context(|| format!("找不到段 {name}")),
            None => Ok((0, self.data.len())),
        }
    }

    /// 在 `[start, end)` 里搜索模式，返回全部命中的**文件偏移**。
    pub fn find_pattern(&self, pat: &[PatByte], range: (usize, usize)) -> Vec<usize> {
        let (start, end) = range;
        let end = end.min(self.data.len());
        let mut hits = Vec::new();
        if pat.is_empty() || end < start || end - start < pat.len() {
            return hits;
        }
        for i in start..=(end - pat.len()) {
            let mut ok = true;
            for (k, p) in pat.iter().enumerate() {
                if !p.matches(self.data[i + k]) {
                    ok = false;
                    break;
                }
            }
            if ok {
                hits.push(i);
            }
        }
        hits
    }

    /// `FindHex`：搜一串十六进制（支持 `??` 通配）。
    pub fn find_hex(&self, hexstr: &str, range: (usize, usize)) -> Result<Vec<usize>> {
        let pat = hex::parse_pattern(hexstr)?;
        Ok(self.find_pattern(&pat, range))
    }

    /// `FindHexN`：第 N 次命中（N 从 1 开始）。
    pub fn find_hex_nth(&self, hexstr: &str, range: (usize, usize), nth: usize) -> Result<Option<usize>> {
        let hits = self.find_hex(hexstr, range)?;
        Ok(hits.get(nth.saturating_sub(1)).copied())
    }

    /// `FindLastHex`：最后一次命中。
    pub fn find_last_hex(&self, hexstr: &str, range: (usize, usize)) -> Result<Option<usize>> {
        let hits = self.find_hex(hexstr, range)?;
        Ok(hits.last().copied())
    }

    /// `FindText`：按指定编码搜字符串（编码后的字节序列）。
    pub fn find_text(&self, needle: &[u8], range: (usize, usize)) -> Vec<usize> {
        if needle.is_empty() {
            return Vec::new();
        }
        let (start, end) = range;
        let end = end.min(self.data.len());
        let mut hits = Vec::new();
        if end < start || end - start < needle.len() {
            return hits;
        }
        let mut i = start;
        while i + needle.len() <= end {
            if &self.data[i..i + needle.len()] == needle {
                hits.push(i);
                i += needle.len();
            } else {
                i += 1;
            }
        }
        hits
    }

    // ---------------------------------------------------------------- 读取

    /// 完整 VA → RVA（小于镜像基址的当 RVA 处理）。
    pub fn rva_of_va(&self, va: u32) -> u32 {
        if va >= self.pe.image_base {
            va - self.pe.image_base
        } else {
            va
        }
    }

    /// RVA → 完整 VA。
    pub fn va_of_rva(&self, rva: u32) -> u32 {
        self.pe.image_base.wrapping_add(rva)
    }

    /// 文件偏移 → 完整 VA。
    pub fn va_of_offset(&self, off: usize) -> Option<u32> {
        self.pe.offset_to_va(off).map(|rva| self.va_of_rva(rva))
    }

    /// VA → 文件偏移（要求落在某个段的 raw 范围内）。
    pub fn offset_of_va(&self, va: u32) -> Result<usize> {
        let rva = self.rva_of_va(va);
        self.pe
            .va_to_offset(rva)
            .with_context(|| format!("VA 0x{va:08X} 不在任何段的原始数据里"))
    }

    /// `GetU8`
    pub fn read_u8_va(&self, va: u32) -> Result<u8> {
        let off = self.offset_of_va(va)?;
        Ok(self.data[off])
    }

    /// `GetU16`（小端）
    pub fn read_u16_va(&self, va: u32) -> Result<u16> {
        let off = self.offset_of_va(va)?;
        ensure!(off + 2 <= self.data.len(), "读 U16 越界：VA 0x{va:08X}");
        Ok(u16::from_le_bytes(
            self.data[off..off + 2].try_into().unwrap(),
        ))
    }

    /// `GetU32`（小端）
    pub fn read_u32_va(&self, va: u32) -> Result<u32> {
        let off = self.offset_of_va(va)?;
        ensure!(off + 4 <= self.data.len(), "读 U32 越界：VA 0x{va:08X}");
        Ok(u32::from_le_bytes(
            self.data[off..off + 4].try_into().unwrap(),
        ))
    }

    // ---------------------------------------------------------------- 写入

    /// 写之前的边界检查：必须在文件内、必须在同一个段的 raw 范围内。
    pub fn check_write(&self, off: usize, len: usize) -> Result<()> {
        ensure!(len > 0, "写入长度为 0");
        ensure!(
            off.checked_add(len).is_some_and(|end| end <= self.data.len()),
            "写入越界：文件偏移 0x{off:X} + {len} 字节超出文件末尾（{} 字节）",
            self.data.len()
        );
        let s = self
            .pe
            .section_of_offset(off)
            .with_context(|| format!("写入位置 0x{off:X} 不在任何段里"))?;
        ensure!(
            off + len <= (s.raw_ptr + s.raw_size) as usize,
            "写入越过段 {} 的末尾（0x{:X} + {} 字节）",
            s.name,
            s.raw_ptr + s.raw_size,
            len
        );
        Ok(())
    }

    /// `SetHex`：在 VA 处写一段字节。
    pub fn write_hex_va(&mut self, va: u32, bytes: &[u8]) -> Result<()> {
        let off = self.offset_of_va(va)?;
        self.write_hex_offset(off, bytes)
    }

    /// `SetHex`（文件偏移版）。
    pub fn write_hex_offset(&mut self, off: usize, bytes: &[u8]) -> Result<()> {
        self.check_write(off, bytes.len())?;
        self.data[off..off + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// `SetU8`
    pub fn write_u8_va(&mut self, va: u32, v: u8) -> Result<()> {
        self.write_hex_va(va, &[v])
    }

    /// `SetU16`（小端）
    pub fn write_u16_va(&mut self, va: u32, v: u16) -> Result<()> {
        self.write_hex_va(va, &v.to_le_bytes())
    }

    /// `SetU32`（小端）
    pub fn write_u32_va(&mut self, va: u32, v: u32) -> Result<()> {
        self.write_hex_va(va, &v.to_le_bytes())
    }

    /// `SetNOPs`：写 len 个 `90`。
    pub fn write_nops_va(&mut self, va: u32, len: usize) -> Result<()> {
        let bytes = vec![0x90u8; len];
        self.write_hex_va(va, &bytes)
    }

    /// `SetJMP`：在 `from` 处写一条跳到 `to` 的相对跳转。
    ///
    /// 优先写 `E9 rel32`（5 字节）；如果 5 字节放不下（越过段尾/文件尾），
    /// 就退化成 `EB rel8`（2 字节）。rel8 距离超出 ±127 时报错。
    pub fn write_jmp_va(&mut self, from: u32, to: u32) -> Result<JmpKind> {
        let off = self.offset_of_va(from)?;
        let rel32 = to as i64 - (from as i64 + 5);
        let rel32_bytes = (rel32 as i32).to_le_bytes();
        // 先试 5 字节的 E9
        if self.check_write(off, 5).is_ok() {
            let mut buf = [0u8; 5];
            buf[0] = 0xE9;
            buf[1..].copy_from_slice(&rel32_bytes);
            self.write_hex_offset(off, &buf)?;
            return Ok(JmpKind::Rel32);
        }
        // 空间不够 → EB rel8
        let rel8 = to as i64 - (from as i64 + 2);
        ensure!(
            (-128..=127).contains(&rel8),
            "跳转距离 {rel8} 超出 rel8 范围，且 5 字节的位置放不下"
        );
        self.check_write(off, 2)?;
        let buf = [0xEBu8, (rel8 as i8) as u8];
        self.write_hex_offset(off, &buf)?;
        Ok(JmpKind::Rel8)
    }

    /// `GetTgtAddr`：解析 `E9` / `EB` / `0F 8x` 跳转，返回目标 VA。
    pub fn get_tgt_addr(&self, va: u32) -> Result<u32> {
        let b0 = self.read_u8_va(va)?;
        match b0 {
            0xE9 => {
                let rel = self.read_u32_va(va + 1)? as i32;
                Ok((va as i64 + 5 + rel as i64) as u32)
            }
            0xEB => {
                let rel = self.read_u8_va(va + 1)? as i8 as i32;
                Ok((va as i64 + 2 + rel as i32 as i64) as u32)
            }
            0x0F => {
                let b1 = self.read_u8_va(va + 1)?;
                if !(0x80..=0x8F).contains(&b1) {
                    bail!("0F {b1:02X} 不是 Jcc 指令");
                }
                let rel = self.read_u32_va(va + 2)? as i32;
                Ok((va as i64 + 6 + rel as i64) as u32)
            }
            other => bail!("0x{other:02X} 不是跳转指令（E9 / EB / 0F 8x）"),
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    /// 造一个最小的假 PE：1 段 .text，raw_ptr=0x200，raw_size=0x200。
    fn fake_engine(payload: &[u8]) -> Engine {
        fake_engine_with(payload, 0x200)
    }

    /// 同上，但可以指定段的 raw 大小（用来测越界）。
    fn fake_engine_with(payload: &[u8], raw_size: u32) -> Engine {
        let mut data = vec![0u8; 0x400];
        // MZ
        data[0] = b'M';
        data[1] = b'Z';
        let e_lfanew = 0x80u32;
        data[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        data[e_lfanew as usize..e_lfanew as usize + 4].copy_from_slice(b"PE\0\0");
        let coff = e_lfanew as usize + 4;
        data[coff..coff + 2].copy_from_slice(&0x14Cu16.to_le_bytes()); // Machine = i386
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        data[coff + 16..coff + 18].copy_from_slice(&0xE0u16.to_le_bytes()); // SizeOfOptionalHeader
        let opt = coff + 20;
        data[opt..opt + 2].copy_from_slice(&0x10Bu16.to_le_bytes()); // PE32
        data[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry
        data[opt + 28..opt + 32].copy_from_slice(&0x400000u32.to_le_bytes()); // image base
        let sec = opt + 0xE0;
        data[sec..sec + 8].copy_from_slice(b".text\0\0\0");
        data[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // vsize
        data[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // va
        data[sec + 16..sec + 20].copy_from_slice(&raw_size.to_le_bytes()); // raw size
        data[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw ptr
        data[0x200..0x200 + payload.len()].copy_from_slice(payload);
        Engine::new(data).unwrap()
    }

    #[test]
    fn 搜十六进制与通配() {
        let e = fake_engine(&[0xB8, 0xDE, 0xBD, 0xC3, 0xC1, 0xF6, 0x00]);
        let hits = e.find_hex("B8 DE BD C3", (0, e.data.len())).unwrap();
        assert_eq!(hits, vec![0x200]);
        let hits = e.find_hex("?? DE BD C3", (0, e.data.len())).unwrap();
        assert_eq!(hits, vec![0x200]);
        let hits = e.find_hex("FF FF", (0, e.data.len())).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn 第N次与最后一次命中() {
        // 三个相同的模式
        let e = fake_engine(&[0xAA, 0xBB, 0x01, 0xAA, 0xBB, 0x02, 0xAA, 0xBB]);
        let range = (0, e.data.len());
        assert_eq!(e.find_hex_nth("AA BB", range, 1).unwrap(), Some(0x200));
        assert_eq!(e.find_hex_nth("AA BB", range, 3).unwrap(), Some(0x206));
        assert_eq!(e.find_hex_nth("AA BB", range, 9).unwrap(), None);
        assert_eq!(e.find_last_hex("AA BB", range).unwrap(), Some(0x206));
    }

    #[test]
    fn 段限定搜索() {
        let e = fake_engine(&[0xAA, 0xBB]);
        let range = e.range_of(Some(".text")).unwrap();
        assert_eq!(range, (0x200, 0x400));
        assert_eq!(e.find_hex("AA BB", range).unwrap(), vec![0x200]);
        assert!(e.find_hex("AA BB", (0, 0x200)).unwrap().is_empty());
        assert!(e.range_of(Some(".nope")).is_err());
    }

    #[test]
    fn 读与写_u32() {
        let mut e = fake_engine(&[0x11, 0x22, 0x33, 0x44]);
        let va = 0x401000u32;
        assert_eq!(e.read_u32_va(va).unwrap(), 0x44332211);
        e.write_u32_va(va, 0xDEADBEEF).unwrap();
        assert_eq!(e.read_u32_va(va).unwrap(), 0xDEADBEEF);
        e.write_u16_va(va, 0x1234).unwrap();
        assert_eq!(e.read_u16_va(va).unwrap(), 0x1234);
        e.write_u8_va(va, 0x99).unwrap();
        assert_eq!(e.read_u8_va(va).unwrap(), 0x99);
    }

    #[test]
    fn 写nops() {
        let mut e = fake_engine(&[0x01, 0x02, 0x03, 0x04]);
        e.write_nops_va(0x401000, 3).unwrap();
        assert_eq!(&e.data[0x200..0x203], &[0x90, 0x90, 0x90]);
        assert_eq!(e.data[0x203], 0x04);
    }

    #[test]
    fn 越界写入被拒绝() {
        let mut e = fake_engine(&[0u8; 8]);
        // 段尾是 0x400，越界写必须失败
        assert!(e.write_hex_offset(0x3FE, &[0u8; 4]).is_err());
        // 超出文件末尾
        assert!(e.write_hex_offset(0x3FF, &[0u8; 8]).is_err());
        // 段外（文件头）
        assert!(e.write_hex_offset(0x10, &[0u8; 4]).is_err());
    }

    #[test]
    fn setjmp_rel32() {
        let mut e = fake_engine(&[0u8; 16]);
        let from = 0x401000u32;
        let to = 0x401100u32;
        assert_eq!(e.write_jmp_va(from, to).unwrap(), JmpKind::Rel32);
        assert_eq!(e.data[0x200], 0xE9);
        let rel = i32::from_le_bytes(e.data[0x201..0x205].try_into().unwrap());
        assert_eq!(from as i64 + 5 + rel as i64, to as i64);
        // 目标地址能解回来
        assert_eq!(e.get_tgt_addr(from).unwrap(), to);
    }

    #[test]
    fn setjmp_rel8_fallback() {
        // 段里只有 2 字节空间，5 字节的 E9 放不下 → 退化成 EB rel8
        let mut e = fake_engine_with(&[0u8; 16], 2);
        let from = 0x401000u32;
        let to = from + 2 + 100; // rel8 = 100
        assert_eq!(e.write_jmp_va(from, to).unwrap(), JmpKind::Rel8);
        assert_eq!(e.data[0x200], 0xEB);
        assert_eq!(e.data[0x201] as i8, 100);
        assert_eq!(e.get_tgt_addr(from).unwrap(), to);
    }

    #[test]
    fn gettgtaddr_解析_jcc() {
        let mut e = fake_engine(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]);
        let va = 0x401000u32;
        e.write_u32_va(va + 2, 0x10).unwrap();
        assert_eq!(e.get_tgt_addr(va).unwrap(), va + 6 + 0x10);
        // 非跳转指令要报错
        assert!(e.get_tgt_addr(va + 6).is_err());
    }

    #[test]
    fn 搜文本() {
        let e = fake_engine(b"hello world\x00abc\x00");
        let hits = e.find_text(b"abc", (0, e.data.len()));
        assert_eq!(hits, vec![0x20C]);
    }
}
