//! `official-palette` —— 所有服务器都用官方（韩服）时装配色。
//!
//! WARP 原版（`OfficialPal.qjs`）：在 `CSession::InitJobTable` 里，
//! 「读语言类型 → 决定用哪套装色前缀」的那个条件跳转（JNE）会被 NOP 掉，
//! 于是配色前缀永远是韩服那套。
//!
//! 本客户端里的对应代码长这样（VC14，2025 版）：
//!
//! ```text
//! cmp  dword ptr [g_serviceType], 0
//! mov  eax, [esi]
//! jne  <跳过整套前缀赋值>          ; ← 这里就是被 NOP 的跳转
//! mov  dword ptr [eax+38h], <"骑士">
//! ```
//!
//! 特征码：先找到配色前缀字符串（CP949 的「骑士」+ 结束符），再找
//! `mov dword ptr [reg+38h], <该字符串>`，最后找它前面 0x20 字节内的
//! 近距 JNE（`0F 85 rel32`，高位是 00 00）。

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// 配色前缀「骑士」（CP949）+ 结束符
const KNIGHT: &[u8] = b"\xC5\xA9\xB7\xE7\x00";

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let rdata = ctx.section(".rdata")?;
    let text = ctx.section(".text")?;

    // 1. 找配色前缀字符串
    let str_hits = ctx.find_bytes(KNIGHT, rdata);
    if str_hits.is_empty() {
        return Ok(Outcome::Fail(
            "exe 里找不到配色前缀「骑士」—— 本版本不适用".to_string(),
        ));
    }

    // 2. 找 `mov dword ptr [reg+38h], <前缀地址>`，并在其前面找近距 JNE
    let mut jne: Option<usize> = None;
    let mut noped: Option<usize> = None;
    for &soff in &str_hits {
        let sva = ctx.va(soff).unwrap_or(0);
        let needle = format!("C7 4? 38 {:02X} {:02X} {:02X} {:02X}", sva & 0xFF, (sva >> 8) & 0xFF, (sva >> 16) & 0xFF, (sva >> 24) & 0xFF);
        let hits = ctx.find(&needle, text)?;
        for &a in &hits {
            // 在赋值指令前面 0x20 字节内找近距 JNE
            let win = (a.saturating_sub(0x20), a);
            let jnes = ctx.find("0F 85 ?? ?? 00 00", win)?;
            if let Some(&j) = jnes.first() {
                jne = Some(j);
                break;
            }
            // 已经被 NOP 掉了？
            let nops = ctx.find("90 90 90 90 90 90", win)?;
            if let Some(&j) = nops.first() {
                noped = Some(j);
            }
        }
        if jne.is_some() || noped.is_some() {
            break;
        }
    }
    if jne.is_none() {
        if let Some(n) = noped {
            ctx.marker = Some(n);
            return Ok(Outcome::Already);
        }
    }

    let Some(jne) = jne else {
        return Ok(Outcome::Fail(
            "找不到「语言类型判断 → 配色前缀赋值」之间的条件跳转 —— 本版本不适用".to_string(),
        ));
    };

    // 3. NOP 掉这个 6 字节的近距 JNE
    if ctx.eng.data[jne..jne + 6].iter().all(|b| *b == 0x90) {
        return Ok(Outcome::Already);
    }
    ctx.nop(jne, 6)?;
    Ok(Outcome::Applied)
}
