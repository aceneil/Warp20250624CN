//! `cutin-esc` —— 按 ESC 可以关掉 cutin（过场动画）窗口。
//!
//! WARP 原版（`CloseCutinOnEsc.qjs`）：在 `UIWindowMgr::ProcessPushButton` 里
//! 「检查能不能关窗」（`call check_close`）的地方挂钩 —— 先试着
//! `DeleteWindow(78h)`（78h 就是 cutin 窗口的 id），关掉了就直接返回；
//! 没关掉就走原来的逻辑。
//!
//! 本客户端（VC14.29）的对应代码：
//!
//! ```text
//! ; UIIllustWnd::SendMsg 里的 case 6
//! cmp  eax, 6
//! je   <case 6>              ; ← 锚点
//! ...
//! <case 6>:
//! cmp  dword ptr [ebp+10h], 0C9h
//! jne  _skip
//! mov  ecx, <g_modeMgr>
//! call CModeMgr::GetGameMode
//! push 0 / push 1 / push -1 / mov edx,[eax] / mov ecx,eax
//! push offset ""
//! push 64h
//! call [edx+18h]
//!
//! ; UIWindowMgr::ProcessPushButton 里的 ESC 检查
//! push <窗口>
//! mov  ecx, <g_uiWindowMgr>    ; ← 挂钩点在这条 call 上
//! call check_close
//! mov  ecx, eax
//! test ecx, ecx
//! je   _skip2
//! ```

use anyhow::{anyhow, Result};

use super::{cave, Ctx, Outcome};
use crate::table::PatchSpec;

/// `UIIllustWnd::SendMsg` 里 case 6 的那个跳转（后面跟着 case 0x22 / case 0x4E）。
const CASE6: &str = "0F 84 ?? ?? 00 00 83 F8 22 0F 84 ?? ?? 00 00 83 F8 4E";
/// case 6 处理函数里的 `CModeMgr::GetGameMode` 调用序列。
const GET_GAME_MODE: &str =
    "B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? 6A 00 6A ?? 6A FF 8B 10 8B C8 68 ?? ?? ?? 00 6A 6? FF 5? 1?";
/// `UIWindowMgr::ProcessPushButton` 里的 ESC 检查。
const ESC_CHECK: &str =
    "5? B9 ?? ?? ?? 0? E? ?? ?? ?? ?? 8B C8 85 C9 0F 84 ?? ?? 00 00";
/// VC14.29 的「默认分支」（cmp reg,-1 / cmp [x],2 / push 0）。
const DEFAULT_CASE: &str =
    "83 F? FF 0F 85 ?? ?? FF FF 83 3D ?? ?? ?? ?? 02 0F 85 ?? ?? FF FF 6A 00";
/// `UIWindowMgr::DeleteWindow` 的调用点。
const DELETE_WINDOW: &str = "5? 8B C? E8 ?? ?? ?? ?? 3C 01";

/// 注入代码长度。
const CODE_LEN: usize = 66;

pub fn apply(ctx: &mut Ctx, _spec: &PatchSpec) -> Result<Outcome> {
    let text = ctx.section(".text")?;

    // 1. case 6 的锚点
    let anchors = ctx.find(CASE6, text)?;
    let Some(&anchor) = anchors.first() else {
        return Ok(Outcome::Fail(
            "找不到 UIIllustWnd::SendMsg 的 case 6 —— 本版本不适用".to_string(),
        ));
    };
    let anchor_va = ctx
        .va(anchor)
        .ok_or_else(|| anyhow!("锚点 0x{anchor:X} 不在文件里"))?;
    let handler = ctx.eng.get_tgt_addr(anchor_va)?;

    // 2. case 6 处理函数里的 GetGameMode 序列
    let handler_off = ctx.eng.offset_of_va(handler)?;
    let win = (handler_off, (handler_off + 0x50).min(text.1));
    let ggm = ctx.find(GET_GAME_MODE, win)?;
    let Some(&g) = ggm.first() else {
        return Ok(Outcome::Fail(
            "找不到 CModeMgr::GetGameMode 的调用序列 —— 本版本不适用".to_string(),
        ));
    };
    // gModeMgr = `mov ecx, <g_modeMgr>` 里的立即数
    let g_mode_mgr = u32::from_le_bytes(ctx.eng.data[g + 1..g + 5].try_into().unwrap());
    // GetGameMode = 那个 call 的目标（call 在 g+5）
    let get_game_mode = ctx.call_target(
        ctx.va(g + 5)
            .ok_or_else(|| anyhow!("GetGameMode 调用点不在文件里"))?,
    )?;
    // suffix = call 之后的那 20 个字节（push 0 / push 1 / push -1 / ... / call [edx+disp]）
    let suffix = ctx.eng.data[g + 10..g + 30].to_vec();
    if suffix.len() != 20 {
        return Ok(Outcome::Fail("GetGameMode 调用序列长度不对 —— 本版本不适用".into()));
    }

    // 3. ESC 检查
    let escs = ctx.find(ESC_CHECK, text)?;
    let Some(&esc) = escs.first() else {
        return Ok(Outcome::Fail(
            "找不到 UIWindowMgr::ProcessPushButton 里的 ESC 检查 —— 本版本不适用".to_string(),
        ));
    };
    let esc_va = ctx
        .va(esc)
        .ok_or_else(|| anyhow!("ESC 检查点 0x{esc:X} 不在文件里"))?;
    // push reg(1) + mov ecx, imm32(5) + call(5) → call 在 esc+6
    let hook_off = esc + 6;
    // 已经打过？hook 点原来是 E8（call），变成 E9（jmp）就是打过
    if ctx.byte(hook_off) == 0xE9 {
        ctx.marker = Some(hook_off);
        return Ok(Outcome::Already);
    }
    if ctx.byte(hook_off) != 0xE8 {
        return Ok(Outcome::Fail(format!(
            "ESC 检查点 0x{hook_off:X} 不是 call 指令（0x{:02X}）—— 本版本不适用",
            ctx.byte(hook_off)
        )));
    }
    let check_close = ctx.call_target(
        ctx.va(hook_off)
            .ok_or_else(|| anyhow!("hook 点不在文件里"))?,
    )?;
    // call 之后的那条指令（= 原 call 的返回地址）
    let retn_vir = esc_va + 11;
    // `mov ecx, <g_uiWindowMgr>`（B9 + imm32）里的立即数（imm32 在 esc+2）
    let this_va = u32::from_le_bytes(ctx.eng.data[esc + 2..esc + 6].try_into().unwrap());

    // 4. VC14.29 的默认分支
    // 窗口要给足：模式本身有 24 字节，得保证它能完整落在窗口里
    let win = (esc, (esc + 0x150 + 32).min(text.1));
    let defs = ctx.find(DEFAULT_CASE, win)?;
    let Some(&def) = defs.first() else {
        return Ok(Outcome::Fail(
            "找不到 VC14.29 的默认分支 —— 本版本不适用".to_string(),
        ));
    };
    let jmp_vir = ctx
        .va(def)
        .ok_or_else(|| anyhow!("默认分支不在文件里"))?;
    // 默认分支里第一个 jne 的目标（= DeleteWindow 调用点）
    let jne_tgt = ctx.eng.get_tgt_addr(
        ctx.va(def + 3)
            .ok_or_else(|| anyhow!("默认分支不在文件里"))?,
    )?;

    // 5. DeleteWindow 的调用点
    let jne_off = ctx.eng.offset_of_va(jne_tgt)?;
    let win = (jne_off, (jne_off + 0x100).min(text.1));
    let dws = ctx.find(DELETE_WINDOW, win)?;
    let Some(&dw) = dws.first() else {
        return Ok(Outcome::Fail(
            "找不到 UIWindowMgr::DeleteWindow 的调用 —— 本版本不适用".to_string(),
        ));
    };
    let delete_window = ctx.call_target(
        ctx.va(dw + 3)
            .ok_or_else(|| anyhow!("DeleteWindow 调用点不在文件里"))?,
    )?;

    // 7. 拼注入代码
    let cave = cave::alloc(ctx.eng, CODE_LEN)?;
    let rel32 = |from_off: u32, target_va: u32| -> [u8; 4] {
        let rel = target_va as i64 - (cave.va as i64 + from_off as i64 + 5);
        (rel as i32).to_le_bytes()
    };

    let mut code: Vec<u8> = Vec::with_capacity(CODE_LEN);
    // ---- part 0：DeleteWindow(78h) 失败 -> 走原来的 check_close ----
    code.extend_from_slice(&[0x6A, 0x78]); // push 78h
    code.extend_from_slice(&[0xE8]); // call DeleteWindow
    code.extend_from_slice(&rel32(2, delete_window));
    code.extend_from_slice(&[0x3C, 0x00]); // cmp al, 0
    code.push(0x75); // jnz part1
    code.push((26 - 11) as u8);
    code.extend_from_slice(&[0xB9]); // mov ecx, <g_uiWindowMgr>
    code.extend_from_slice(&this_va.to_le_bytes());
    code.extend_from_slice(&[0x68]); // push retnVir
    code.extend_from_slice(&retn_vir.to_le_bytes());
    code.extend_from_slice(&[0xE9]); // jmp check_close
    code.extend_from_slice(&rel32(21, check_close));

    // ---- part 1：DeleteWindow 成功 -> 播完动画再回到 DeleteWindow 调用点 ----
    code.push(0x52); // push edx
    code.extend_from_slice(&[0xB9]); // mov ecx, <g_modeMgr>
    code.extend_from_slice(&g_mode_mgr.to_le_bytes());
    code.extend_from_slice(&[0xE8]); // call GetGameMode
    code.extend_from_slice(&rel32(32, get_game_mode));
    code.extend_from_slice(&[0x8B, 0x10]); // mov edx, [eax]
    code.extend_from_slice(&suffix); // 原来的那段调用
    code.push(0x5A); // pop edx
    code.push(0x58); // pop eax
    code.extend_from_slice(&[0xE9]); // jmp <DeleteWindow 调用点>
    code.extend_from_slice(&rel32(61, jmp_vir));
    debug_assert_eq!(code.len(), CODE_LEN);

    ctx.write(cave.offset, &code)?;

    // 8. 把原来的 call check_close 改成 jmp 注入代码
    let hook_va = ctx
        .va(hook_off)
        .ok_or_else(|| anyhow!("hook 点不在文件里"))?;
    let rel = cave.va as i64 - (hook_va as i64 + 5);
    let mut jmp = vec![0xE9];
    jmp.extend_from_slice(&(rel as i32).to_le_bytes());
    ctx.write(hook_off, &jmp)?;

    Ok(Outcome::Applied)
}
