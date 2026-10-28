//! `load-kr-extsettings` —— 一律加载韩服的 `ExternalSettings_kr` lua 配置。
//!
//! WARP 原版（`LoadKrExtSettings.qjs`）：找到 `ExternalSettings_kr` 字符串被 push
//! 的地方，往前找 `g_serviceType` 的比较，然后写一条 JMP 跳过比较，无条件走
//! 「加载韩服配置」那条路。
//!
//! 这个客户端的结构不一样了：它在 `CSession` 里按服务类型（韩/美/日/中/台）
//! 从一个跳转表选出一串 `mov ecx, "<Lua Files\service_xxx\ExternalSettings_yyy>"`
//! 来加载配置。所以这里的做法等价：**把所有 `ExternalSettings` 配置路径的指针
//! 都改指到韩服那份**（`Lua Files\service_korea\ExternalSettings_kr`）。
//!
//! 找法：扫 `.text` 里所有 `mov reg, <立即数>`，看那个立即数指向的字符串是不是
//! `Lua Files\service_*\ExternalSettings*`；是的话就改指韩服那份。

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// 目标路径：韩服
const KOREA: &str = "Lua Files\\service_korea\\ExternalSettings_kr";

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let text = ctx.section(".text")?;
    let data = ctx.eng.data.clone();

    // 1. 找韩服那份路径字符串（必须整串完全相等，不能是别的变体的前缀）
    let mut korea_va: Option<u32> = None;
    let needle = format!("{KOREA}\0");
    let mut i = 0usize;
    while let Some(p) = data[i..]
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())
        .map(|p| i + p)
    {
        // 必须是字符串的起点：前一个字节是 0 或者在段首
        let starts_here = p == 0 || data[p - 1] == 0;
        if starts_here {
            korea_va = ctx.va(p);
            break;
        }
        i = p + 1;
    }
    let Some(korea_va) = korea_va else {
        return Ok(Outcome::Fail(format!(
            "exe 里找不到「{KOREA}」—— 本版本不适用"
        )));
    };

    // 2. 扫 .text，找所有指向 `Lua Files\service_*\ExternalSettings*` 的 mov 立即数
    let mut targets: Vec<usize> = Vec::new(); // mov 指令的文件偏移
    for off in text.0..text.1.saturating_sub(5) {
        let op = data[off];
        if op != 0xB8 && op != 0xB9 && op != 0xBA {
            continue;
        }
        let imm = u32::from_le_bytes(data[off + 1..off + 5].try_into().unwrap());
        let Ok(soff) = ctx.eng.offset_of_va(imm) else {
            continue;
        };
        if soff + 12 > data.len() {
            continue;
        }
        let s = &data[soff..soff + 96];
        let end = s.iter().position(|b| *b == 0).unwrap_or(s.len());
        let s = String::from_utf8_lossy(&s[..end]).to_string();
        if !s.starts_with("Lua Files\\service_") || !s.contains("ExternalSettings") {
            continue;
        }
        if s.starts_with("Lua Files\\service_korea") {
            continue; // 已经是韩服的，不动
        }
        targets.push(off);
    }

    if targets.is_empty() {
        // 没有别的服务器要改：看看是不是已经改过了（韩服那份有没有被引用）
        let kneedle = format!(
            "B9 {:02X} {:02X} {:02X} {:02X}",
            korea_va & 0xFF,
            (korea_va >> 8) & 0xFF,
            (korea_va >> 16) & 0xFF,
            (korea_va >> 24) & 0xFF
        );
        let k = ctx.find(&kneedle, text)?;
        if let Some(&koff) = k.first() {
            ctx.marker = Some(koff);
            return Ok(Outcome::Already);
        }
        return Ok(Outcome::Fail(
            "找不到指向其它服务器 ExternalSettings 的引用 —— 本版本不适用".to_string(),
        ));
    }

    // 3. 全部改指韩服
    let mut applied = 0usize;
    let mut already = 0usize;
    for off in targets {
        let cur = u32::from_le_bytes(data[off + 1..off + 5].try_into().unwrap());
        if cur == korea_va {
            already += 1;
            continue;
        }
        ctx.write(off + 1, &korea_va.to_le_bytes())?;
        applied += 1;
    }

    if applied == 0 {
        Ok(Outcome::Already)
    } else {
        let _ = already;
        Ok(Outcome::Applied)
    }
}
