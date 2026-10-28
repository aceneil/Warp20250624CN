//! `no-gm-sprite` —— 不显示 GM 精灵，但保留黄字 / 管理员右键菜单等功能。
//!
//! WARP 原版（`NoGMSprite.qjs`）：找到 `CPc::SetActNameList` 和
//! `CPc::SetSprNameList` 这两个函数里「调用 IsNameYellow（判断是不是 GM 账号）」
//! 之后的那个条件跳转（`JZ`），把它改成 `JMP` —— 也就是永远走「不是 GM」的分支，
//! 于是 GM 精灵不会被画出来，但其它 GM 相关功能原封不动。
//!
//! 特征码（VC14.29 专属的写法）：
//!
//! ```text
//! ; CPc::SetSprNameList / SetActNameList 的调用点
//! push eax            ; 名字指针
//! push 5              ; layer
//! call CPc::SetActNameList
//! ```
//!
//! 函数内部：
//!
//! ```text
//! push dword ptr [edi+110h]   ; accountID
//! call IsNameYellow
//! add  esp, 4
//! test al, al
//! jz   <不是 GM 的分支>
//! ```

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// VC14.29：GM 账号 ID 分发的那段 switch（WARP 用它定位两个调用点）。
const ANCHOR_1429: &str = "83 E8 0C 74 ?? 2D A1 0F 00 00 74 ?? 83 E8 16 74 ?? EB ?? \
                            2D E1 0F 00 00 74 ?? 83 E8 24 74 ?? 2D 99 00 00 00";

/// `push eax; push 5; call <函数>` —— 后面的立即数是往后的偏移。
const CALL_SITES: &[&str] = &[
    // 后面跟 push -1
    "50 6A 05 E8 ?? ?? ?? FF 6A FF",
    // 后面跟 lea reg, [reg-1]
    "50 6A 05 E8 ?? ?? ?? FF 8D 4? FF",
    // 后面跟 cmp dword ptr [ebp+disp32], 10h
    "50 6A 05 E8 ?? ?? ?? FF 83 BD ?? ?? ?? ?? 10",
];

/// `call xxx; add esp, 4; test al, al`
const IS_YELLOW: &str = "E8 ?? ?? ?? ?? 83 C4 04 84 C0";

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let text = ctx.section(".text")?;

    // 1. 找 14.29 的锚点（switch on GM id）
    let anchors = ctx.find(ANCHOR_1429, text)?;
    let Some(&anchor) = anchors.first() else {
        return Ok(Outcome::Fail(
            "找不到 GM 账号 ID 分发的那段代码（VC14.29 特征）—— 本版本不适用".to_string(),
        ));
    };
    // 锚点后面紧跟的 2 字节是 switch 的跳转表入口，WARP 也是这么跳过去的
    let after = anchor + 36 + 2;

    // 2. 在后面找两个「push eax; push 5; call ...」
    let win = (after + 0x30, (after + 0xA0).min(text.1));
    let mut sites: Vec<usize> = Vec::new();
    for sig in CALL_SITES {
        for h in ctx.find(sig, win)? {
            if !sites.contains(&h) {
                sites.push(h);
            }
        }
    }
    sites.sort_unstable();
    if sites.len() != 2 {
        return Ok(Outcome::Fail(format!(
            "在 GM ID 分发代码后面找到 {} 个 Set*NameList 调用点（期望 2 个）—— 本版本不适用",
            sites.len()
        )));
    }

    // 3. 两个函数里找「call IsNameYellow; add esp,4; test al,al; jz」
    //    用两个窗口里 call 目标的交集来认出 IsNameYellow。
    let mut per_func: Vec<Vec<(usize, u32)>> = Vec::new();
    for &site in &sites {
        let call_va = ctx
            .va(site + 3)
            .ok_or_else(|| anyhow::anyhow!("调用点 0x{:X} 不在文件里", site + 3))?;
        let func_va = ctx.call_target(call_va)?;
        let func_off = ctx.eng.offset_of_va(func_va)?;
        let w = (
            func_off.saturating_sub(0x80),
            (func_off + 0x80).min(text.1),
        );
        let mut cands = Vec::new();
        for p in ctx.find(IS_YELLOW, w)? {
            let call_va = ctx
                .va(p)
                .ok_or_else(|| anyhow::anyhow!("0x{:X} 不在文件里", p))?; // E8 的位置
            let target = ctx.call_target(call_va)?;
            cands.push((p, target));
        }
        if cands.is_empty() {
            return Ok(Outcome::Fail(
                "在 Set*NameList 里找不到「call IsNameYellow; add esp,4; test al,al」—— 本版本不适用"
                    .to_string(),
            ));
        }
        per_func.push(cands);
    }

    // 两个函数调用的是同一个 IsNameYellow → 取交集
    let common: Option<u32> = per_func[0]
        .iter()
        .map(|(_, t)| *t)
        .find(|t| per_func[1].iter().any(|(_, t2)| t2 == t));
    let Some(is_yellow) = common else {
        return Ok(Outcome::Fail(
            "两个 Set*NameList 里的 IsNameYellow 不是同一个函数 —— 本版本不适用".to_string(),
        ));
    };

    // 4. 把 JZ 改成 JMP（目标不变）
    let mut applied = 0usize;
    let mut already = 0usize;
    let mut marker: Option<usize> = None;
    for cands in &per_func {
        let Some(&(pat_off, _)) = cands.iter().find(|(_, t)| *t == is_yellow) else {
            continue;
        };
        let jz_off = pat_off + 10; // CALL(5) + add esp,4(3) + test al,al(2)
        let b0 = ctx.byte(jz_off);
        if b0 == 0xEB || b0 == 0xE9 {
            already += 1;
            marker.get_or_insert(jz_off);
            continue;
        }
        match b0 {
            // 短跳转 JZ rel8 -> JMP rel8（位移不变）
            0x74 => {
                ctx.write(jz_off, &[0xEB, ctx.byte(jz_off + 1)])?;
                applied += 1;
            }
            // 近距 JZ rel32 -> JMP rel32（多出的 1 字节用 NOP 补上）
            0x0F if ctx.byte(jz_off + 1) == 0x84 => {
                let rel = i32::from_le_bytes(
                    ctx.eng.data[jz_off + 2..jz_off + 6].try_into().unwrap(),
                );
                let new_rel = rel + 1; // E9 比 0F 84 少 1 字节
                ctx.write(
                    jz_off,
                    &[
                        0xE9,
                        new_rel.to_le_bytes()[0],
                        new_rel.to_le_bytes()[1],
                        new_rel.to_le_bytes()[2],
                        new_rel.to_le_bytes()[3],
                        0x90,
                    ],
                )?;
                applied += 1;
            }
            other => {
                return Ok(Outcome::Fail(format!(
                    "IsNameYellow 之后的跳转不是 JZ（是 0x{other:02X}）—— 本版本不适用"
                )));
            }
        }
    }

    if applied == 0 && already > 0 {
        ctx.marker = marker;
        Ok(Outcome::Already)
    } else {
        Ok(Outcome::Applied)
    }
}
