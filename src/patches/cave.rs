//! 代码洞（code cave）分配器。
//!
//! 有些补丁（无边框全屏、ESC 关 cutin）需要往 exe 里塞一段自己的代码。
//! 客户端 PE 里 `.text` 段末尾和下一段之间有一段**文件内的空洞**（PE 段表里
//! `.text` 的 `SizeOfRawData` 比实际填满的少一截），这段空洞：
//!
//! * 在文件里存在（是 0），但不属于任何段；
//! * 在内存里正好落在 `.text` 的虚拟地址范围内 → **可执行**。
//!
//! 所以：把 `.text` 的 `SizeOfRawData` 往后面撑一点（不会挪动任何段的数据），
//! 就能得到一块又大又安全的可执行空间。

use anyhow::{bail, Result};

use crate::engine::Engine;

/// 分配到的代码洞。
#[derive(Debug, Clone, Copy)]
pub struct Cave {
    /// 文件偏移。
    pub offset: usize,
    /// 虚拟地址（完整 VA）。
    pub va: u32,
    /// 可用长度。
    #[allow(dead_code)]
    pub size: usize,
}

/// 在 `.text` 段后面的文件空洞里分配 `size` 字节。
///
/// * 找不到足够大的空洞 → 报错（报告里会写「找不到特征/空间」）。
/// * 空洞剩余部分会填 `0xCC`（int3），这样下一次分配不会误选到本次写过的区域。
pub fn alloc(eng: &mut Engine, size: usize) -> Result<Cave> {
    // 1. 找 .text 段和它后面那个段的 raw 起点
    let text_idx = eng
        .pe
        .sections
        .iter()
        .position(|s| s.name == ".text")
        .ok_or_else(|| anyhow::anyhow!("这个 exe 没有 .text 段，没地方放注入的代码"))?;
    let (text_raw_ptr, text_raw_size, text_va) = {
        let s = &eng.pe.sections[text_idx];
        (s.raw_ptr as usize, s.raw_size as usize, s.va)
    };
    let gap_start = text_raw_ptr + text_raw_size;
    let gap_end = eng
        .pe
        .sections
        .iter()
        .filter(|s| s.raw_ptr as usize > gap_start)
        .map(|s| s.raw_ptr as usize)
        .min()
        .unwrap_or(eng.data.len());
    if gap_end <= gap_start {
        bail!(
            ".text 段后面没有文件空洞（raw_ptr={text_raw_ptr:X} raw_size={text_raw_size:X}），\
             没法放注入的代码"
        );
    }
    if gap_end - gap_start < size {
        bail!(
            ".text 段后面的文件空洞只有 {} 字节，不够 {} 字节",
            gap_end - gap_start,
            size
        );
    }

    // 2. 在空洞里找一段连续的 0
    let mut run_start = None;
    let mut run_len = 0usize;
    let mut found: Option<usize> = None;
    for i in gap_start..gap_end {
        if eng.data[i] == 0 {
            if run_start.is_none() {
                run_start = Some(i);
            }
            run_len += 1;
            if run_len >= size {
                found = run_start;
                break;
            }
        } else {
            run_start = None;
            run_len = 0;
        }
    }
    let Some(off) = found else {
        bail!(
            ".text 段后面的文件空洞（0x{gap_start:X}..0x{gap_end:X}）里没有 {size} 字节的连续 0"
        );
    };

    // 3. 把 .text 的 SizeOfRawData 撑到「代码结束」为止
    //    （只撑到代码末尾，不动后面段的数据，也方便下一次分配接着用这段空洞）
    let code_end = off + size;
    let new_raw_size = (code_end - text_raw_ptr) as u32;
    if new_raw_size > text_raw_size as u32 {
        eng.pe.sections[text_idx].raw_size = new_raw_size;
        // 段表在文件里也要一起改（SizeOfRawData 在段头的 +16 处）
        let hdr = eng.pe.section_table_offset + text_idx * 40 + 16;
        if hdr + 4 <= eng.data.len() {
            eng.data[hdr..hdr + 4].copy_from_slice(&new_raw_size.to_le_bytes());
        }
    }

    // 4. 代码后面垫一小段 0xCC（int3），避免以后再被当成「空闲的 0」
    //    —— 只垫 16 字节，剩下的空洞留给下一条补丁用
    let guard_end = (code_end + 16).min(gap_end);
    for b in &mut eng.data[code_end..guard_end] {
        *b = 0xCC;
    }

    // 完整 VA = 镜像基址 + 段 RVA + 段内偏移
    let va = eng.pe.image_base + text_va + ((off - text_raw_ptr) as u32);
    Ok(Cave {
        offset: off,
        va,
        size,
    })
}
