//! `no-kro-site` —— 禁止客户端在改完游戏内设置后跳到 ro.gnjoy.com 等网站。
//!
//! WARP 原版（`NoGNJoyLaunch.qjs`）的做法：把 URL 字符串的**第一个字节清零**，
//! 字符串就变成空串，客户端拿不到可打开的网址。
//!
//! 这里照抄 WARP 的候选 URL 列表（少一个就少一个，全都清零）。

use anyhow::Result;

use super::{Ctx, Outcome};
use crate::table::PatchSpec;

/// WARP 里列的全部候选 URL。
const URLS: &[&str] = &[
    "ro.gnjoy.com",
    "http://ro.gnjoy.com",
    "http://ro.gnjoy.com/",
    "http://www.ragnarok.co.in/index.php",
    "https://iro.ragnarokonline.com/member/memberjoin1.asp?mNum=1",
    "http://t.kunlun.com/?act=voucher.main&cid=1000&pid=61",
    "http://www.hangame.com",
    "http://ragnarok.co.kr",
    "http://roz.gnjoy.com",
    "http://roz.gnjoy.com/",
];

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let range = ctx.all();
    // 同一个地址可能被多个候选 URL 命中（比如 "ro.gnjoy.com" 是
    // "http://ro.gnjoy.com/" 的子串），去重。
    let mut targets: Vec<usize> = Vec::new();
    let mut found_any = false;
    let mut done_any = false;
    let mut done_at: Option<usize> = None;

    for url in URLS {
        let hits = ctx.find_bytes(url.as_bytes(), range);
        let Some(&off) = hits.first() else {
            // 找不到了？看看是不是第一个字节已经被清零（= 已经打过）
            let mut zeroed = url.as_bytes().to_vec();
            zeroed[0] = 0x00;
            let z = ctx.find_bytes(&zeroed, range);
            if let Some(&zoff) = z.first() {
                done_any = true;
                done_at.get_or_insert(zoff);
            }
            continue;
        };
        found_any = true;
        if !targets.contains(&off) {
            targets.push(off);
        }
    }

    if !found_any && !done_any {
        return Ok(Outcome::Fail(
            "exe 里一个候选 URL 都没找到 —— 本版本不适用".to_string(),
        ));
    }

    let mut applied = 0usize;
    let mut already = 0usize;
    let first_target = targets.first().copied();
    for off in targets {
        if ctx.byte(off) == 0x00 {
            already += 1;
            continue;
        }
        ctx.write(off, &[0x00])?;
        applied += 1;
    }

    if applied == 0 {
        ctx.marker = done_at.or(first_target);
        let _ = already;
        Ok(Outcome::Already)
    } else {
        let _ = already;
        Ok(Outcome::Applied)
    }
}
