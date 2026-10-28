//! `map-quality` —— 地图贴图用 32 位色（提高画质）。
//!
//! WARP 原版（`IncrMapQuality.qjs`）：找到 `CTexMgr::CreateTexture` 的调用，
//! 然后把调用前那个 `push 1`（pf 参数 = 1，16 位色）改成 `push 4`（32 位色）。
//!
//! 特征码（VC14 起的写法）：
//!
//! ```text
//! mov  ecx, <g_texMgr>
//! push eax            ; imgData
//! push 100h           ; h
//! push 100h           ; w
//! call CTexMgr::CreateTexture
//! ```

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// 各种编译器版本下 `CreateTexture` 调用点的特征。
const SIGS: &[&str] = &[
    // VC14: mov ecx, <g_texMgr>; push reg; push 100h; push 100h; call
    "B9 ?? ?? ?? ?? 50 68 00 01 00 00 68 00 01 00 00 E8",
    "8B 0D ?? ?? ?? ?? 50 68 00 01 00 00 68 00 01 00 00 E8",
    // VC10~VC13: push reg; push 100h; push 100h; mov ecx, <g_texMgr>; call
    "50 68 00 01 00 00 68 00 01 00 00 B9 ?? ?? ?? ?? E8",
    "8B 0D ?? ?? ?? ?? 68 00 01 00 00 68 00 01 00 00 E8",
    // VC6~VC9: push ecx; push 100h; push 100h; mov ecx, <g_texMgr>; call
    "51 68 00 01 00 00 68 00 01 00 00 B9 ?? ?? ?? ?? E8",
];

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let range = ctx.section(".text")?;
    let mut hit: Option<usize> = None;
    for sig in SIGS {
        let hits = ctx.find(sig, range)?;
        if let Some(&h) = hits.first() {
            hit = Some(h);
            break;
        }
    }
    let Some(start) = hit else {
        return Ok(Outcome::Fail(
            "找不到 CTexMgr::CreateTexture 的调用点 —— 本版本不适用".to_string(),
        ));
    };

    // pf 参数（push 1）在调用点前面 10 字节以内
    let win_start = start.saturating_sub(10);
    let pushes = ctx.find("6A 01", (win_start, start))?;
    let Some(&p) = pushes.first() else {
        // 已经是 push 4 了？
        let done = ctx.find("6A 04", (win_start, start))?;
        if let Some(&q) = done.first() {
            ctx.marker = Some(q);
            return Ok(Outcome::Already);
        }
        return Ok(Outcome::Fail(format!(
            "找到了 CreateTexture 调用（偏移 0x{start:X}），但前面没有 push 1（pf 参数）—— 本版本不适用"
        )));
    };

    if ctx.byte(p + 1) == 0x04 {
        ctx.marker = Some(p);
        return Ok(Outcome::Already);
    }
    // push 1 -> push 4（32 位色）
    ctx.write(p + 1, &[0x04])?;
    Ok(Outcome::Applied)
}
