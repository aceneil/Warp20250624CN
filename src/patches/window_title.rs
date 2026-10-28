//! `window-title` —— 窗口标题的「优先原地写，放不下就搬」。
//!
//! M3a 的 `window_title` 只能写 11 字节（`.rdata` 里那段
//! `"Ragnarok\0\0\0\0"` 的匹配区减去 1 字节结束符），超了就报「太长」。
//! WARP 的同名补丁支持 60 字符，做法是**把标题搬走**：
//!
//! ```text
//! 标题写进 .text 段尾的空洞
//!   → 把引用标题的那几条指令的立即数改成新地址
//! ```
//!
//! 本客户端里引用窗口标题的指令有两条：
//!
//! ```text
//! 0x00733363: B8 04 26 FF 00                     mov eax, 0x00FF2604
//! 0x00A59DD0: C7 05 80 D8 1E 01 04 26 FF 00       mov dword ptr [0x011ED880], 0x00FF2604
//! ```
//!
//! 两条都是「立即数里存着字符串地址」，改成新地址就行 ——
//! 指令长度不变、栈平衡不变、分支不变。
//!
//! ## 两条路径各自的条件
//!
//! | 路径 | 条件 | 报告里写什么 |
//! |---|---|---|
//! | 原地写 | 标题编码后 ≤ 匹配区字节数 − 1（结束符） | 「原地写」+ 落点偏移 |
//! | 搬 | 上面那条不成立 | 「搬进空洞」+ 新字符串 VA + 每条被改的指令 |
//!
//! 两条路径都幂等：原地写看落点字节，搬看空洞里的标记（魔数 + 原标题 VA）。

use anyhow::Result;

use super::cave;
use super::relocate::{self, RefKind};
use super::{Ctx, Outcome};
use crate::encode;
use crate::hex;
use crate::table::PatchSpec;

/// 引用标题字符串的指令形式（`mov eax, imm32` / `mov [mem], imm32` 也要认）。
const REF_KINDS: [RefKind; 3] = [RefKind::Push, RefKind::MovReg, RefKind::MovMem];

pub fn apply(ctx: &mut Ctx, spec: &PatchSpec) -> Result<Outcome> {
    // ---------------- 1. 标题（ask 参数） ----------------
    let key = spec
        .ask
        .as_ref()
        .map(|a| a.key.clone())
        .unwrap_or_else(|| "title".to_string());
    let Some(title) = ctx.params.get(&key).cloned() else {
        return Ok(Outcome::Fail(format!(
            "补丁 {} 没有参数 {key} 的值（用 --set {}=... 传）",
            ctx.id, key
        )));
    };

    // ---------------- 2. 编码 + 结束符 ----------------
    let bytes = encode::encode_for(&title, ctx.encoding, &key)?;
    let mut desired = bytes.clone();
    desired.push(0x00);

    // ---------------- 3. 找原标题字符串 ----------------
    if spec.find.trim().is_empty() {
        return Ok(Outcome::Fail(format!("补丁 {} 没有声明 find 特征", ctx.id)));
    }
    let pat = hex::parse_pattern(&spec.find)?;
    let sec = spec.section.as_deref().unwrap_or(".rdata");
    let range = ctx.section(sec)?;
    let hits = ctx.eng.find_pattern(&pat, range);
    if hits.is_empty() {
        // 原串找不到了 → 多半是「原地写」已经打过了（原地写会把原串覆盖掉）
        // 幂等兜底：段里已经有这个标题了就当打过
        if let Some(off) = ctx.eng.find_text(&desired, range).into_iter().next() {
            ctx.marker = Some(off);
            ctx.record(
                off,
                desired.clone(),
                desired.clone(),
                Some(format!(
                    "原地写：标题「{title}」已经写在 {sec} 的 0x{off:08X}（{} 字节，含结束符）",
                    desired.len()
                )),
            );
            ctx.summary = Some(format!(
                "原地写：标题「{title}」已经写在 0x{off:08X}（{} 字节）",
                desired.len()
            ));
            return Ok(Outcome::Already);
        }
        return Ok(Outcome::Fail(format!(
            "找不到特征 —— 本版本不适用：{sec} 里找不到窗口标题原串"
        )));
    }
    let off = hits[0];
    let Some(old_va) = ctx.va(off) else {
        return Ok(Outcome::Fail("窗口标题原串不在任何段里".to_string()));
    };

    // ---------------- 4. 原地写还是搬？ ----------------
    let zero_fill = spec.zero_fill.unwrap_or(false);
    let pad = nul_padding(&ctx.eng.data, off + pat.len());
    let limit = if zero_fill {
        pat.len()
    } else {
        pat.len() + pad
    };

    if desired.len() <= limit {
        // ---- 原地写 ----
        let mut buf = desired.clone();
        if zero_fill {
            buf.resize(pat.len(), 0x00);
        }
        let cur = &ctx.eng.data[off..off + buf.len()];
        if cur == buf.as_slice() {
            ctx.marker = Some(off);
            ctx.record(
                off,
                cur.to_vec(),
                buf.clone(),
                Some(format!(
                    "原地写：标题已经是「{title}」（{} 字节，含结束符），落在 {sec} 的 0x{off:08X}",
                    desired.len()
                )),
            );
            ctx.summary = Some(format!(
                "原地写：标题「{title}」已经写在 0x{off:08X}（{} 字节，含结束符）",
                desired.len()
            ));
            return Ok(Outcome::Already);
        }
        ctx.write(off, &buf)?;
        ctx.summary = Some(format!(
            "原地写：标题「{title}」写进 {sec} 的 0x{off:08X}（{} 字节，含结束符；匹配区 {limit} 字节，放得下）",
            desired.len()
        ));
        return Ok(Outcome::Applied);
    }

    // ---- 放不下 → 搬进空洞 ----
    let text_range = ctx.section(".text")?;

    // 上次搬的地方？（用户换过标题会留下多个标记，只认「还被引用着」的那个）
    for mark_off in relocate::find_cave_marks(&ctx.eng, old_va) {
        let mark_va = ctx.va(mark_off).unwrap_or(0);
        let str_va = relocate::cave_string_va(mark_va);
        let refs = relocate::find_imm_refs(&ctx.eng, str_va, &REF_KINDS, text_range);
        if !refs.is_empty() {
            let cur = relocate::string_at_va(&ctx.eng, str_va).ok().flatten();
            // desired 末尾带结束符，cur 不带 → 比前 (len-1) 字节
            if cur.as_deref() == Some(&desired[..desired.len() - 1]) {
                ctx.marker = Some(refs[0].op_offset);
                for r in &refs {
                    let imm = &ctx.eng.data[r.imm_offset..r.imm_offset + 4];
                    let old = u32::from_le_bytes(imm.try_into().unwrap());
                    ctx.record(
                        r.imm_offset,
                        imm.to_vec(),
                        imm.to_vec(),
                        Some(format!(
                            "{} @0x{:08X}：imm32 0x{old:08X} → 0x{str_va:08X}（已指向新标题）",
                            r.kind.name(),
                            r.op_offset
                        )),
                    );
                }
                ctx.record(
                    mark_off + relocate::CAVE_MARK_LEN,
                    desired.clone(),
                    desired.clone(),
                    Some(format!(
                        "搬进空洞的标题：VA 0x{str_va:08X}，字节 {}",
                        relocate::hex_str(&desired)
                    )),
                );
                ctx.summary = Some(format!(
                    "搬进空洞：标题「{title}」已经在 0x{str_va:08X}（{} 字节）",
                    desired.len()
                ));
                return Ok(Outcome::Already);
            }
            // 用户换了标题 → 把这些引用点改指到新的副本
            return relocate_to(ctx, &refs, old_va, &desired, &title);
        }
    }

    // 找引用原标题的指令
    let refs = relocate::find_imm_refs(&ctx.eng, old_va, &REF_KINDS, text_range);
    if refs.is_empty() {
        return Ok(Outcome::Fail(format!(
            "找不到特征 —— 本版本不适用：没有任何指令引用窗口标题字符串 0x{old_va:08X}"
        )));
    }
    relocate_to(ctx, &refs, old_va, &desired, &title)
}

/// 把标题写进空洞，并改掉 `refs` 里那些指令的立即数。
fn relocate_to(
    ctx: &mut Ctx,
    refs: &[relocate::ImmRef],
    old_va: u32,
    desired: &[u8],
    title: &str,
) -> Result<Outcome> {
    let need = relocate::CAVE_MARK_LEN + desired.len();
    let cave = cave::alloc(ctx.eng, need)?;
    let mark = relocate::cave_mark_bytes(old_va);
    ctx.write_note(
        cave.offset,
        &mark,
        format!(
            "搬字符串的标记（魔数 + 原标题 VA 0x{old_va:08X}）：第二次运行靠它认出自己上次搬的地方"
        ),
    )?;
    let str_off = cave.offset + relocate::CAVE_MARK_LEN;
    let str_va = cave.va + relocate::CAVE_MARK_LEN as u32;
    ctx.write_note(
        str_off,
        desired,
        format!(
            "搬过去的标题：VA 0x{str_va:08X}，字节 {}（含结束符）",
            relocate::hex_str(desired)
        ),
    )?;

    let mut lines = Vec::new();
    for r in refs {
        let old = u32::from_le_bytes(ctx.eng.data[r.imm_offset..r.imm_offset + 4].try_into().unwrap());
        ctx.write_note(
            r.imm_offset,
            &str_va.to_le_bytes(),
            format!(
                "{} 指令 @0x{:08X}（{} 字节，没变）：imm32 0x{old:08X} → 0x{str_va:08X}",
                r.kind.name(),
                r.op_offset,
                r.insn_len
            ),
        )?;
        lines.push(format!(
            "{} @0x{:08X}：imm32 0x{old:08X} → 0x{str_va:08X}",
            r.kind.name(),
            r.op_offset
        ));
    }
    ctx.summary = Some(format!(
        "搬进空洞：标题「{title}」→ 新字符串 VA 0x{str_va:08X}（{} 字节）；{}",
        desired.len(),
        lines.join("；")
    ));
    Ok(Outcome::Applied)
}

/// 紧跟在 `from` 之后的连续 0x00 个数（借用写入预算）。
fn nul_padding(data: &[u8], from: usize) -> usize {
    let mut n = 0usize;
    let mut i = from;
    while i < data.len() && data[i] == 0x00 {
        n += 1;
        i += 1;
    }
    n
}
