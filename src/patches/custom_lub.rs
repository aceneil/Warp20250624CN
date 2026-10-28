//! `custom-lub` —— WARP 的 `Custom*Lub` 六条（指令级改写）。
//!
//! | 我们的 id | WARP id | 原路径（示例） | 默认新路径 |
//! |---|---|---|---|
//! | `custom-achieve-lub` | `CustomAchieveLub` | `AchievementsList.lub` | `SystemCN\AchievementsList.lub` |
//! | `custom-mapinfo-lub` | `CustomMapInfoLub` | `MapInfo.lub` | `SystemCN\MapInfo.lub` |
//! | `custom-ongoingquest-lub` | `CustomOngQuestInfoLub` | `OngoingQuestInfoList*.lub` | `SystemCN\OngoingQuests` |
//! | `custom-recommendedquest-lub` | `CustomRcmdQuestInfoLub` | `RecommendedQuestInfoList*.lub` | `SystemCN\RecommendedQuests_basic` |
//! | `custom-tipbox-lub` | `CustomTipboxLub` | `Tipbox.lub` | `SystemCN\Tipbox.lub` |
//! | `custom-towninfo-lub` | `CustomTownInfoLub` | `Towninfo.lub` | `SystemCN\Towninfo.lub` |
//!
//! ## 它和普通 find/replace 补丁的区别
//!
//! 普通补丁改的是**数据**；这六条改的是**指令**：
//!
//! ```text
//! 找 PUSH imm32（imm32 = 原路径字符串的 VA）
//!   → 把新路径字符串写进 .text 段尾的空洞（见 super::cave）
//!   → 把 PUSH 的 imm32 改成新字符串的 VA
//! ```
//!
//! 所以指令本身一个字节都不用动，栈平衡 / 寄存器 / 分支全都保持原样 ——
//! 变的只是那个立即数指向哪里。
//!
//! ## 定位
//!
//! * 原路径靠补丁表里的 `path_old`（大小写不敏感，`*` 通配）在本客户端里找；
//! * 引用点靠 `push <字符串VA>`（opcode 0x68）找。
//!
//! 找不到就明确报「找不到特征 —— 本版本不适用」，**绝不猜偏移**。
//!
//! ## 幂等
//!
//! 搬进空洞的时候会在字符串前面写一段标记（魔数 + 原字符串 VA），
//! 第二次运行靠这个标记找回自己上次搬的地方：
//!
//! * 标记还在、而且那儿的字符串就是要写的新路径 → `already`；
//! * 标记还在、但字符串是别的（用户换了路径）→ 把引用点改指到新的副本。

use anyhow::Result;

use super::cave;
use super::relocate;
use super::{Ctx, Outcome};
use crate::encode;
use crate::table::PatchSpec;

pub fn apply(ctx: &mut Ctx, spec: &PatchSpec) -> Result<Outcome> {
    // ---------------- 1. 新路径（ask 参数，可被 --set 覆盖） ----------------
    let key = spec
        .ask
        .as_ref()
        .map(|a| a.key.clone())
        .unwrap_or_else(|| "path".to_string());
    let Some(new_path) = ctx.params.get(&key).cloned() else {
        return Ok(Outcome::Fail(format!(
            "补丁 {} 没有参数 {key} 的值（用 --set {}=... 传）",
            ctx.id, key
        )));
    };

    // ---------------- 2. 按落地编码转字节 ----------------
    let bytes = encode::encode_for(&new_path, ctx.encoding, &key)?;
    let mut desired = bytes.clone();
    desired.push(0x00); // 结束符
    if desired.len() > relocate::MAX_PATH_BYTES {
        return Ok(Outcome::Fail(format!(
            "参数 {key} 的路径太长（{} 字节，含结束符；最多 {} 字节）",
            desired.len(),
            relocate::MAX_PATH_BYTES
        )));
    }

    // ---------------- 3. 找原路径字符串 ----------------
    let Some(pattern) = spec.path_old.as_deref() else {
        return Ok(Outcome::Fail(format!(
            "补丁 {} 没有声明 path_old（原路径特征）",
            ctx.id
        )));
    };
    let sec = spec.section.as_deref().unwrap_or(".rdata");
    let range = ctx.section(sec)?;
    let olds = relocate::find_strings(&ctx.eng, pattern, range);
    if olds.is_empty() {
        return Ok(Outcome::Fail(format!(
            "找不到特征 —— 本版本不适用：{sec} 里没有匹配「{pattern}」的路径字符串"
        )));
    }
    let old_va = olds[0].va;
    let text_range = ctx.section(".text")?;

    // ---------------- 4. 上次搬的地方（标记） ----------------
    // 标记 = 魔数 + 原字符串 VA，所以别的补丁不会认错。
    // 用户换过路径的话会留下多个标记 —— 只认「还被 push 引用着」的那个。
    for mark_off in relocate::find_cave_marks(&ctx.eng, old_va) {
        let mark_va = ctx.va(mark_off).unwrap_or(0);
        let str_va = relocate::cave_string_va(mark_va);
        let sites = relocate::find_push_sites(&ctx.eng, str_va, text_range);
        if sites.is_empty() {
            // 旧落点，已经没人引用了（用户换过路径）
            continue;
        }
        let cur = relocate::string_at_va(&ctx.eng, str_va).ok().flatten();
        // desired 末尾带结束符，cur 不带 → 比前 (len-1) 字节
        if cur.as_deref() == Some(&desired[..desired.len() - 1]) {
            // 已经是目标状态
            ctx.marker = Some(sites[0]);
            let note = format!(
                "已经搬好了：空洞里的新字符串 VA 0x{str_va:08X}，字节 {}",
                relocate::hex_str(&desired)
            );
            for &s in &sites {
                let imm = &ctx.eng.data[s + 1..s + 5];
                let old = u32::from_le_bytes(imm.try_into().unwrap());
                ctx.record(
                    s + 1,
                    imm.to_vec(),
                    imm.to_vec(),
                    Some(format!(
                        "原 PUSH 指令 @0x{:08X}（opcode 68 push imm32）；imm32 0x{old:08X} → 0x{str_va:08X}（已指向新字符串）",
                        s
                    )),
                );
            }
            ctx.record(
                mark_off + relocate::CAVE_MARK_LEN,
                desired.clone(),
                desired.clone(),
                Some(note.clone()),
            );
            ctx.summary = Some(note);
            return Ok(Outcome::Already);
        }
        // 用户换了路径 → 把这些引用点改指到新的副本
        return relocate_to(ctx, &sites, old_va, &desired, &new_path, &olds);
    }

    // ---------------- 5. 找引用原字符串的 PUSH ----------------
    let mut sites: Vec<usize> = Vec::new();
    for s in &olds {
        let ps = relocate::find_push_sites(&ctx.eng, s.va, text_range);
        if ps.is_empty() {
            return Ok(Outcome::Fail(format!(
                "找不到特征 —— 本版本不适用：没有任何指令 push「{}」的地址",
                s.text
            )));
        }
        sites.extend(ps);
    }

    relocate_to(ctx, &sites, old_va, &desired, &new_path, &olds)
}

/// 把新路径写进空洞，并把 `sites` 里的 PUSH 立即数改指过去。
fn relocate_to(
    ctx: &mut Ctx,
    sites: &[usize],
    old_va: u32,
    desired: &[u8],
    new_path: &str,
    olds: &[relocate::FoundString],
) -> Result<Outcome> {
    // 分配空洞（标记 + 字符串）
    let need = relocate::CAVE_MARK_LEN + desired.len();
    let cave = cave::alloc(ctx.eng, need)?;
    let mark = relocate::cave_mark_bytes(old_va);
    ctx.write_note(
        cave.offset,
        &mark,
        format!(
            "搬字符串的标记（魔数 + 原字符串 VA 0x{old_va:08X}）：第二次运行靠它认出自己上次搬的地方"
        ),
    )?;
    let str_off = cave.offset + relocate::CAVE_MARK_LEN;
    let str_va = cave.va + relocate::CAVE_MARK_LEN as u32;
    ctx.write_note(
        str_off,
        desired,
        format!(
            "新路径字符串：VA 0x{str_va:08X}，字节 {}（含结束符）",
            relocate::hex_str(desired)
        ),
    )?;

    let mut lines = Vec::new();
    for &s in sites {
        let old = u32::from_le_bytes(ctx.eng.data[s + 1..s + 5].try_into().unwrap());
        ctx.write_note(
            s + 1,
            &str_va.to_le_bytes(),
            format!(
                "原 PUSH 指令 @0x{s:08X}（opcode 68 push imm32，指令 5 字节没变）；imm32 0x{old:08X} → 0x{str_va:08X}"
            ),
        )?;
        lines.push(format!(
            "原 PUSH @0x{s:08X}：imm32 0x{old:08X} → 0x{str_va:08X}"
        ));
    }

    let summary = format!(
        "原路径「{}」（0x{:08X}）→ 新路径「{new_path}」；新字符串 VA 0x{str_va:08X}（空洞 0x{:08X}），字符串字节 {}；{}",
        olds.first().map(|s| s.text.as_str()).unwrap_or(""),
        olds.first().map(|s| s.offset).unwrap_or(0),
        cave.offset,
        relocate::hex_str(desired),
        lines.join("；")
    );
    ctx.summary = Some(summary);
    Ok(Outcome::Applied)
}
