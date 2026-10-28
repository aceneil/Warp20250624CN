//! `no-gravity-ads` —— 去掉登录画面的 Gravity 广告贴图。
//!
//! WARP 原版（`NoGravityAds.qjs`）的做法：在 exe 里找 `\T_xxx.tga` 这种贴图名，
//! 把开头的 `\T_` 里的 `T` 改成 `0`，这样客户端就找不到这张图了。
//!
//! 三个目标（客户端里都是 CP949 编码的韩文）：
//!
//! | 串 | 说明 |
//! |---|---|
//! | `\T_중력성인.tga` | 「重力成人」广告图 |
//! | `\T_테입%d.tga` | 「逃亡」广告图 |
//! | `\T_GameGrade.tga` | 游戏分级图标 |

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// `\T_중력성인.tga`（CP949）
const T1: &[u8] = b"\\T_\xC1\xDF\xB7\xC2\xBC\xBA\xC0\xCE.tga";
/// `\T_테입%d.tga`（CP949）
const T2: &[u8] = b"\\T_\xC5\xD7\xC0\xD4%d.tga";
/// `\T_GameGrade.tga`
const T3: &[u8] = b"\\T_GameGrade.tga";

const TARGETS: &[(&str, &[u8])] = &[("중력성인", T1), ("테입", T2), ("GameGrade", T3)];

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let range = ctx.all();
    let mut applied = 0usize;
    let mut already = 0usize;
    let mut missing: Vec<&str> = Vec::new();
    let mut marker: Option<usize> = None;

    for (name, pat) in TARGETS {
        // 已经被改过的样子：`\0_xxx.tga`
        let mut done: Vec<u8> = pat.to_vec();
        done[1] = 0x00;
        let hits = ctx.find_bytes(pat, range);
        if hits.is_empty() {
            // 找不到了？看看是不是已经打过（`T` 已经被清零）
            let z = ctx.find_bytes(&done, range);
            if let Some(&zoff) = z.first() {
                already += 1;
                marker.get_or_insert(zoff);
            } else {
                missing.push(name);
            }
            continue;
        }
        let off = hits[0];
        if ctx.byte(off + 1) == 0x00 {
            already += 1;
            continue;
        }
        ctx.write(off + 1, &[0x00])?;
        applied += 1;
    }

    if applied == 0 && already > 0 {
        ctx.marker = marker;
        return Ok(Outcome::Already);
    }
    if applied == 0 && already == 0 {
        return Ok(Outcome::Fail(format!(
            "exe 里找不到任何 Gravity 广告贴图名（{}）—— 本版本不适用",
            missing.join("、")
        )));
    }
    if !missing.is_empty() {
        return Ok(Outcome::Applied);
    }
    Ok(Outcome::Applied)
}
