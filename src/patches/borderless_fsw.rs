//! `borderless-fsw` —— 无边框全屏（分辨率 == 屏幕分辨率时去掉窗口边框）。
//!
//! WARP 原版（`BorderlessFSW.qjs` + `WinMaker.qjs`）：在 `CreateWindowExA` 的调用点
//! 挂一段自己的代码 —— 如果请求的宽高正好等于屏幕宽高，就把窗口样式改成
//! `WS_POPUP | WS_VISIBLE`、坐标改成 (0,0)，这样看起来就是无边框全屏。
//!
//! 注入的代码放在 `.text` 段后面那截文件空洞里（见 [`super::cave`]），
//! 原调用点改成 `call [注入代码]`。
//!
//! 注入代码（栈上就是 `CreateWindowExA` 的参数）：
//!
//! ```text
//! push 0                      ; SM_CXSCREEN
//! call [GetSystemMetrics]
//! cmp  [esp+1Ch], eax         ; nWidth
//! jl   _next
//! mov  [tmp], eax
//! push 1                      ; SM_CYSCREEN
//! call [GetSystemMetrics]
//! cmp  [esp+20h], eax         ; nHeight
//! jl   _next
//! mov  [esp+20h], eax
//! mov  eax, [tmp]
//! mov  [esp+1Ch], eax
//! mov  [esp+10h], 90000000h   ; WS_POPUP | WS_VISIBLE
//! xor  eax, eax
//! mov  [esp+14h], eax         ; X = 0
//! mov  [esp+18h], eax         ; Y = 0
//! jmp  _next
//! tmp: dd 0
//! _next:
//! jmp  [CreateWindowExA]
//! ```

use anyhow::{anyhow, Result};

use super::{cave, Ctx, Outcome};
use crate::pe;
use crate::table::PatchSpec;

/// 注入代码长度（含末尾的 tmp 槽和 jmp）。
const CODE_LEN: usize = 76;
/// `_next` 在注入代码里的偏移。
const NEXT_OFF: u32 = 70;
/// `tmp` 槽在注入代码里的偏移。
const TMP_OFF: u32 = 66;

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let text = ctx.section(".text")?;

    // 1. 导入表里找这两个 API 的 IAT 槽
    let create_iat = pe::find_import_iat(&ctx.eng.data, &ctx.eng.pe, "user32.dll", "CreateWindowExA")
        .ok_or_else(|| anyhow!("exe 的导入表里没有 CreateWindowExA"))?;
    let gsm_iat = pe::find_import_iat(&ctx.eng.data, &ctx.eng.pe, "user32.dll", "GetSystemMetrics")
        .ok_or_else(|| anyhow!("exe 的导入表里没有 GetSystemMetrics"))?;

    // 2. 找主窗口的创建点。
    //    特征：`call [CreateWindowExA]` 紧跟着 `mov dword ptr [全局], eax`
    //    （A3 xx xx xx xx）。调用目标写成通配，这样打过补丁之后特征还在。
    let sites = ctx.find("FF 15 ?? ?? ?? ?? A3 ?? ?? ?? ??", text)?;
    // 只认「调用目标 = CreateWindowExA 的 IAT 槽」或者「调用目标 = .text 里的
    // 注入代码（= 已经打过补丁）」的调用点
    let mut main_sites: Vec<usize> = Vec::new();
    let mut already_site: Option<usize> = None;
    let text_lo = ctx.eng.pe.image_base + ctx.eng.pe.sections[0].va;
    let text_hi = text_lo + ctx.eng.pe.sections[0].vsize.max(ctx.eng.pe.sections[0].raw_size);
    for &s in &sites {
        let Some(va) = ctx.va(s + 2) else { continue };
        let Ok(op) = ctx.eng.read_u32_va(va) else {
            continue;
        };
        if op == create_iat {
            main_sites.push(s);
        } else if op >= text_lo && op < text_hi {
            already_site = Some(s);
        }
    }
    if main_sites.is_empty() && already_site.is_some() {
        ctx.marker = already_site;
        return Ok(Outcome::Already);
    }
    if main_sites.is_empty() {
        return Ok(Outcome::Fail(
            "exe 里找不到「创建主窗口」的 CreateWindowExA 调用 —— 本版本不适用".to_string(),
        ));
    }

    // 3. 必须只有一个主窗口创建点
    if main_sites.len() != 1 {
        return Ok(Outcome::Fail(format!(
            "找到 {} 个「创建主窗口」的 CreateWindowExA 调用（期望 1 个）—— 本版本不适用",
            main_sites.len()
        )));
    }
    let site = main_sites[0];

    // 5. 分配代码洞并拼注入代码
    let cave = cave::alloc(ctx.eng, CODE_LEN)?;
    let tmp_va = cave.va + TMP_OFF;

    let mut code: Vec<u8> = Vec::with_capacity(CODE_LEN);
    // push 0 ; call [GetSystemMetrics]
    code.extend_from_slice(&[0x6A, 0x00]);
    code.extend_from_slice(&[0xFF, 0x15]);
    code.extend_from_slice(&gsm_iat.to_le_bytes());
    // cmp [esp+1Ch], eax ; jl _next
    code.extend_from_slice(&[0x3B, 0x44, 0x24, 0x1C]);
    code.push(0x7C);
    code.push((NEXT_OFF - 14) as u8);
    // mov [tmp], eax
    code.extend_from_slice(&[0xA3]);
    code.extend_from_slice(&tmp_va.to_le_bytes());
    // push 1 ; call [GetSystemMetrics]
    code.extend_from_slice(&[0x6A, 0x01]);
    code.extend_from_slice(&[0xFF, 0x15]);
    code.extend_from_slice(&gsm_iat.to_le_bytes());
    // cmp [esp+20h], eax ; jl _next
    code.extend_from_slice(&[0x3B, 0x44, 0x24, 0x20]);
    code.push(0x7C);
    code.push((NEXT_OFF - 33) as u8);
    // mov [esp+20h], eax
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x20]);
    // mov eax, [tmp]
    code.extend_from_slice(&[0xA1]);
    code.extend_from_slice(&tmp_va.to_le_bytes());
    // mov [esp+1Ch], eax
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x1C]);
    // mov [esp+10h], 90000000h
    code.extend_from_slice(&[0xC7, 0x44, 0x24, 0x10, 0x00, 0x00, 0x00, 0x09]);
    // xor eax, eax ; mov [esp+14h], eax ; mov [esp+18h], eax
    code.extend_from_slice(&[0x33, 0xC0]);
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x14]);
    code.extend_from_slice(&[0x89, 0x44, 0x24, 0x18]);
    // jmp _next
    code.push(0xEB);
    code.push((NEXT_OFF - 66) as u8);
    // tmp: dd 0
    code.extend_from_slice(&[0, 0, 0, 0]);
    // _next: jmp [CreateWindowExA]
    code.extend_from_slice(&[0xFF, 0x25]);
    code.extend_from_slice(&create_iat.to_le_bytes());
    debug_assert_eq!(code.len(), CODE_LEN);

    ctx.write(cave.offset, &code)?;

    // 6. 把原来的调用改成 call [注入代码]
    let mut newcall = vec![0xFF, 0x15];
    newcall.extend_from_slice(&cave.va.to_le_bytes());
    ctx.write(site, &newcall)?;

    Ok(Outcome::Applied)
}
