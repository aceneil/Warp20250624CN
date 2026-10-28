//! 极简 PE32/PE32+ 解析：只做补丁引擎需要的那点事。
//!
//! * 段表（名 / 虚拟地址 / 虚拟大小 / 文件偏移 / 文件大小）
//! * 编译时间戳（WARP 的 `Exe.BuildDate`）
//! * VA ↔ 文件偏移互转（WARP 的 `Exe.Phy`）
//! * 镜像基址、入口点

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub va: u32,
    pub vsize: u32,
    pub raw_ptr: u32,
    pub raw_size: u32,
}

impl Section {
    /// 这个 VA 是否落在本段内。
    pub fn contains_va(&self, va: u32) -> bool {
        let end = self.va.saturating_add(self.vsize.max(self.raw_size));
        va >= self.va && va < end
    }
}

#[derive(Debug, Clone)]
pub struct Pe {
    pub image_base: u32,
    pub entry_point: u32,
    pub build_date: u32,
    pub is_64: bool,
    pub sections: Vec<Section>,
    /// 段表在文件里的偏移（改 SizeOfRawData 时要用）。
    pub section_table_offset: usize,
}

impl Pe {
    pub fn parse(data: &[u8]) -> Result<Pe> {
        if data.len() < 0x40 || &data[0..2] != b"MZ" {
            bail!("不是 PE 文件（缺少 MZ 头）");
        }
        let e_lfanew = u32::from_le_bytes(data[0x3C..0x40].try_into().unwrap()) as usize;
        if data.len() < e_lfanew + 24 || &data[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
            bail!("不是 PE 文件（缺少 PE 签名）");
        }
        let coff = e_lfanew + 4;
        let num_sections = u16::from_le_bytes(data[coff + 2..coff + 4].try_into().unwrap()) as usize;
        let build_date = u32::from_le_bytes(data[coff + 4..coff + 8].try_into().unwrap());
        let opt_size = u16::from_le_bytes(data[coff + 16..coff + 18].try_into().unwrap()) as usize;

        let opt = coff + 20;
        let magic = u16::from_le_bytes(data[opt..opt + 2].try_into().unwrap());
        let is_64 = match magic {
            0x10B => false,
            0x20B => true,
            other => bail!("未知的可选头 magic：0x{other:X}"),
        };
        let entry_point = u32::from_le_bytes(data[opt + 16..opt + 20].try_into().unwrap());
        let image_base = if is_64 {
            // PE32+ 用 8 字节基址，低 4 字节足够我们做加法（客户端都在 32 位低地址）
            u32::from_le_bytes(data[opt + 24..opt + 28].try_into().unwrap())
        } else {
            u32::from_le_bytes(data[opt + 28..opt + 32].try_into().unwrap())
        };

        let sec_start = opt + opt_size;
        let mut sections = Vec::with_capacity(num_sections);
        for i in 0..num_sections {
            let off = sec_start + i * 40;
            if data.len() < off + 40 {
                bail!("段表越界");
            }
            let name_raw = &data[off..off + 8];
            let name = String::from_utf8_lossy(
                &name_raw[..name_raw.iter().position(|b| *b == 0).unwrap_or(8)],
            )
            .to_string();
            sections.push(Section {
                name,
                vsize: u32::from_le_bytes(data[off + 8..off + 12].try_into().unwrap()),
                va: u32::from_le_bytes(data[off + 12..off + 16].try_into().unwrap()),
                raw_size: u32::from_le_bytes(data[off + 16..off + 20].try_into().unwrap()),
                raw_ptr: u32::from_le_bytes(data[off + 20..off + 24].try_into().unwrap()),
            });
        }
        Ok(Pe {
            image_base,
            entry_point,
            build_date,
            is_64,
            sections,
            section_table_offset: sec_start,
        })
    }

    /// VA → 文件偏移（WARP 的 `Exe.Phy`）。找不到返回 None。
    pub fn va_to_offset(&self, va: u32) -> Option<usize> {
        for s in &self.sections {
            if s.contains_va(va) {
                let delta = va - s.va;
                if delta < s.raw_size {
                    return Some((s.raw_ptr + delta) as usize);
                }
            }
        }
        None
    }

    /// 文件偏移 → VA。
    pub fn offset_to_va(&self, off: usize) -> Option<u32> {
        let off = off as u32;
        for s in &self.sections {
            if off >= s.raw_ptr && off < s.raw_ptr + s.raw_size {
                return Some(s.va + (off - s.raw_ptr));
            }
        }
        None
    }

    /// 某个段名对应的 [起始文件偏移, 结束文件偏移)。段不存在就返回 None。
    pub fn section_range(&self, name: &str) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .find(|s| s.name == name)
            .map(|s| {
                (
                    s.raw_ptr as usize,
                    (s.raw_ptr + s.raw_size) as usize,
                )
            })
    }

    /// 编译时间的 YYYYMMDD 整数（WARP 补丁表里的 startDate / endDate 就是这个格式）。
    pub fn build_date_ymd(&self) -> u32 {
        let (y, m, d) = civil_from_days((self.build_date as u64 / 86400) as i64);
        (y as u32) * 10000 + m * 100 + d
    }

    /// 某个文件偏移落在哪个段里。
    pub fn section_of_offset(&self, off: usize) -> Option<&Section> {
        let off = off as u32;
        self.sections
            .iter()
            .find(|s| off >= s.raw_ptr && off < s.raw_ptr + s.raw_size)
    }

    /// 某个 VA 落在哪个段里。
    #[allow(dead_code)]
    pub fn section_of_va(&self, va: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains_va(va))
    }

    /// 编译时间戳的可读形式（客户端 build date 用）。
    pub fn build_date_str(&self) -> String {
        // 简单换算：不做完整日历库，够看年份即可
        let secs = self.build_date as u64;
        let days = secs / 86400;
        // 1970-01-01 起算的近似日期（客户端只关心年月日差别不大）
        let (y, m, d) = civil_from_days(days as i64);
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// 在导入表里找「某个 DLL 的某个函数」的 IAT 槽地址（完整 VA）。
///
/// * `dll` / `func` 大小写不敏感。
/// * 找不到返回 `None`。
///
/// IAT 槽就是 `FF 15 <slot>` 这种间接调用指令里用的那个地址。
pub fn find_import_iat(data: &[u8], pe: &Pe, dll: &str, func: &str) -> Option<u32> {
    // 数据目录在可选头里的偏移（PE32 是 +96，PE32+ 是 +112）
    let e_lfanew = u32::from_le_bytes(data[0x3C..0x40].try_into().unwrap()) as usize;
    let coff = e_lfanew + 4;
    let opt = coff + 20;
    let dd_off = if pe.is_64 { opt + 112 } else { opt + 96 };
    let imp_rva = u32::from_le_bytes(data[dd_off + 8..dd_off + 12].try_into().unwrap());
    if imp_rva == 0 {
        return None;
    }

    let rva_to_off = |rva: u32| -> Option<usize> {
        for s in &pe.sections {
            let end = s.va.saturating_add(s.vsize.max(s.raw_size));
            if rva >= s.va && rva < end {
                let d = (rva - s.va) as usize;
                if d < s.raw_size as usize {
                    return Some((s.raw_ptr + d as u32) as usize);
                }
            }
        }
        None
    };
    let cstr = |off: usize| -> String {
        let end = data[off..].iter().position(|b| *b == 0).map(|p| off + p).unwrap_or(off);
        String::from_utf8_lossy(&data[off..end]).to_string()
    };

    let mut desc = rva_to_off(imp_rva)?;
    loop {
        if desc + 20 > data.len() {
            return None;
        }
        let oft = u32::from_le_bytes(data[desc..desc + 4].try_into().unwrap());
        let name_rva = u32::from_le_bytes(data[desc + 12..desc + 16].try_into().unwrap());
        let ft = u32::from_le_bytes(data[desc + 16..desc + 20].try_into().unwrap());
        if oft == 0 && name_rva == 0 && ft == 0 {
            break;
        }
        let dll_name = cstr(rva_to_off(name_rva)?);
        if dll_name.eq_ignore_ascii_case(dll) {
            // 优先用 OriginalFirstThunk（导入名表），没有就用 FirstThunk
            let thunk_rva = if oft != 0 { oft } else { ft };
            let mut t = rva_to_off(thunk_rva)?;
            let mut i = 0usize;
            loop {
                let v = u32::from_le_bytes(data[t..t + 4].try_into().unwrap());
                if v == 0 {
                    break;
                }
                if v & 0x8000_0000 == 0 {
                    let noff = rva_to_off(v)?;
                    let fname = cstr(noff + 2);
                    if fname.eq_ignore_ascii_case(func) {
                        return Some(pe.image_base + ft + (i as u32) * 4);
                    }
                }
                t += 4;
                i += 1;
            }
            return None;
        }
        desc += 20;
    }
    None
}

/// Howard Hinnant 的 civil_from_days（对外版，给日期过滤用）。
pub fn civil_from_days_pub(z: i64) -> (i64, u32, u32) {
    civil_from_days(z)
}

/// Howard Hinnant 的 civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 读 PE，顺便把「是不是加壳/被压缩」的粗判结果带出来。
pub fn analyze(data: &[u8]) -> Result<(Pe, Vec<String>)> {
    let pe = Pe::parse(data).with_context(|| "解析 PE 失败")?;
    let mut notes = Vec::new();
    if let Some((_, end)) = pe.section_range(".text") {
        if end > data.len() {
            notes.push(format!(
                ".text 段越界（段表说 {end}，文件只有 {}）→ 可能加壳或截断",
                data.len()
            ));
        }
    }
    // 入口点落在最后一段、或段名很怪 → 常见加壳特征（UPX/ASPack/Themida）
    let names: Vec<&str> = pe.sections.iter().map(|s| s.name.as_str()).collect();
    for shell in ["UPX0", "UPX1", ".aspack", ".adata", ".themida", ".vmp0", ".vmp1"] {
        if names.iter().any(|n| n.eq_ignore_ascii_case(shell)) {
            notes.push(format!("发现加壳段 {shell} → 必须先脱壳"));
        }
    }
    Ok((pe, notes))
}
