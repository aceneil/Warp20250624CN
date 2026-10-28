//! 自定义补丁处理器。
//!
//! 补丁表里写 `mode: custom` + `handler: <名字>` 就会走到这里。
//! 表驱动的补丁（能写成「找特征 → 改字节」的）应该优先写进 `patches/base.yml`，
//! 只有「多处联动 / 相对跳转 / 立即数改写 / 需要注入代码」才放到这个目录里。
//!
//! 每个处理器都遵守同样的规矩：
//!
//! * **只用特征码定位**，不写死偏移（换客户端版本时要能明确报「找不到特征」）。
//! * **幂等**：已经是目标状态就返回 [`Outcome::Already`]，不重复写。
//! * 每一次写都记进 [`Ctx::writes`]，报告里能看到 old → new。

pub mod borderless_fsw;
pub mod cave;
pub mod cutin_esc;
pub mod load_kr_extsettings;
pub mod map_quality;
pub mod no_gm_sprite;
pub mod no_gravity_ads;
pub mod no_kro_site;
pub mod official_palette;

use anyhow::{anyhow, Result};

use crate::engine::Engine;
use crate::table::PatchSpec;

/// 一次写入的记录（报告里 old → new）。
#[derive(Debug, Clone)]
pub struct Write {
    /// 文件偏移。
    pub offset: usize,
    /// 写之前的字节。
    pub old: Vec<u8>,
    /// 写进去的字节。
    pub new: Vec<u8>,
}

/// 补丁执行上下文：引擎 + 写入记录。
pub struct Ctx<'a> {
    pub eng: &'a mut Engine,
    /// 已经写下去的字节。
    pub writes: Vec<Write>,
    /// 「已经是目标状态」时，报告里要显示的位置。
    pub marker: Option<usize>,
    /// 只检查不真写（`verify` 用）。
    pub check_only: bool,
}

impl<'a> Ctx<'a> {
    pub fn new(eng: &'a mut Engine) -> Self {
        Ctx {
            eng,
            writes: Vec::new(),
            marker: None,
            check_only: false,
        }
    }

    /// 在文件偏移处写一段字节（自动记录 old → new）。
    pub fn write(&mut self, off: usize, bytes: &[u8]) -> Result<()> {
        self.eng.check_write(off, bytes.len())?;
        let old = self.eng.data[off..off + bytes.len()].to_vec();
        if !self.check_only {
            self.eng.write_hex_offset(off, bytes)?;
        }
        self.writes.push(Write {
            offset: off,
            old,
            new: bytes.to_vec(),
        });
        Ok(())
    }

    /// 在虚拟地址处写一段字节。
    #[allow(dead_code)]
    pub fn write_va(&mut self, va: u32, bytes: &[u8]) -> Result<()> {
        let off = self
            .eng
            .offset_of_va(va)
            .map_err(|_| anyhow!("VA 0x{va:08X} 不在文件的原始数据里"))?;
        self.write(off, bytes)
    }

    /// 写 N 个 `90`。
    pub fn nop(&mut self, off: usize, len: usize) -> Result<()> {
        self.write(off, &vec![0x90u8; len])
    }

    /// 搜特征串，返回文件偏移。
    pub fn find(&self, pat: &str, range: (usize, usize)) -> Result<Vec<usize>> {
        self.eng.find_hex(pat, range)
    }

    /// 搜字节串，返回文件偏移。
    pub fn find_bytes(&self, needle: &[u8], range: (usize, usize)) -> Vec<usize> {
        self.eng.find_text(needle, range)
    }

    /// 整个文件。
    pub fn all(&self) -> (usize, usize) {
        (0, self.eng.data.len())
    }

    /// 某个段的 raw 范围。
    pub fn section(&self, name: &str) -> Result<(usize, usize)> {
        self.eng.range_of(Some(name))
    }

    /// 文件偏移 → VA。
    pub fn va(&self, off: usize) -> Option<u32> {
        self.eng.va_of_offset(off)
    }

    /// `call`（E8）的目标地址。
    pub fn call_target(&self, va: u32) -> Result<u32> {
        let b = self.eng.read_u8_va(va)?;
        if b != 0xE8 {
            anyhow::bail!("0x{va:08X} 处不是 call 指令（是 0x{b:02X}）");
        }
        let rel = self.eng.read_u32_va(va + 1)? as i32;
        Ok((va as i64 + 5 + rel as i64) as u32)
    }

    /// 读一个字节。
    pub fn byte(&self, off: usize) -> u8 {
        self.eng.data[off]
    }
}

/// 处理器的结果。
#[derive(Debug, Clone)]
pub enum Outcome {
    /// 本次打上了。
    Applied,
    /// 已经是目标状态（幂等）。
    Already,
    /// 失败（附原因）。
    Fail(String),
    /// 跳过（附原因）。
    #[allow(dead_code)]
    Skipped(String),
}

/// 按名字分发到具体的处理器。
pub fn dispatch(name: &str, ctx: &mut Ctx, spec: &PatchSpec) -> Result<Outcome> {
    match name {
        "borderless-fsw" => borderless_fsw::apply(ctx, spec),
        "cutin-esc" => cutin_esc::apply(ctx, spec),
        "load-kr-extsettings" => load_kr_extsettings::apply(ctx, spec),
        "map-quality" => map_quality::apply(ctx, spec),
        "no-gm-sprite" => no_gm_sprite::apply(ctx, spec),
        "no-gravity-ads" => no_gravity_ads::apply(ctx, spec),
        "no-kro-site" => no_kro_site::apply(ctx, spec),
        "official-palette" => official_palette::apply(ctx, spec),
        other => Err(anyhow!("没有这个自定义补丁处理器：{other}")),
    }
}
