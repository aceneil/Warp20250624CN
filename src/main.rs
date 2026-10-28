//! warp_rs ——  客户端补丁引擎（WARP 核心能力的 Rust 重写）。
//!
//! ```text
//! warp_rs info   --exe <path>
//! warp_rs find   --exe <path> --hex "<hex>" [--section .data] [--nth N] [--last]
//! warp_rs apply  --exe <in> --patches <yml> --out <out> [--report <json>] [--dry-run]
//! warp_rs verify --exe <patched> --patches <yml> [--json]
//! ```

mod apply;
mod encode;
mod engine;
mod hex;
mod pe;
mod table;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use apply::ApplyOptions;
use encode::Encoding;

#[derive(Parser)]
#[command(name = "warp_rs", about = " 客户端补丁引擎（WARP 核心的 Rust 实现）")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 看一个 exe 的 PE 信息（版本时间、段表、加壳特征）
    Info {
        #[arg(long)]
        exe: PathBuf,
    },
    /// 在 exe 里搜一串十六进制（支持 ?? 通配）或一段文本
    Find {
        #[arg(long)]
        exe: PathBuf,
        /// 例如 "B8 DE BD C3 C1 F6"
        #[arg(long, conflicts_with = "text")]
        hex: Option<String>,
        /// 搜文本（配合 --encoding）
        #[arg(long, conflicts_with = "hex")]
        text: Option<String>,
        /// 文本编码：gbk / cp949 / utf8（默认 gbk）
        #[arg(long)]
        encoding: Option<String>,
        /// 只看某个段（如 .data）
        #[arg(long)]
        section: Option<String>,
        /// 第 N 次命中（1 开始）
        #[arg(long)]
        nth: Option<usize>,
        /// 最后一次命中
        #[arg(long)]
        last: bool,
        /// 最多显示多少条
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 读一段数据（GetU8/U16/U32 / GetTgtAddr）
    Read {
        #[arg(long)]
        exe: PathBuf,
        /// 虚拟地址（完整 VA，例如 0x00401000）
        #[arg(long, value_parser = parse_int)]
        va: u32,
        /// 读多少字节（默认 4）
        #[arg(long, default_value_t = 4)]
        len: usize,
        /// 按 1 字节读
        #[arg(long)]
        u8: bool,
        /// 按 2 字节读
        #[arg(long)]
        u16: bool,
        /// 按 4 字节读
        #[arg(long)]
        u32: bool,
        /// 解析该处的跳转目标（E9 / EB / 0F 8x）
        #[arg(long)]
        tgt: bool,
    },
    /// 写一段数据（SetU8/U16/U32 / SetHex / SetNOPs / SetJMP）
    Write {
        /// 输入 exe
        #[arg(long)]
        exe: PathBuf,
        /// 输出 exe
        #[arg(long)]
        out: PathBuf,
        /// 虚拟地址（完整 VA）
        #[arg(long, value_parser = parse_int)]
        va: u32,
        /// 写这些字节，例如 "90 90"
        #[arg(long, conflicts_with_all = ["nops", "jmp", "set_u32"])]
        hex: Option<String>,
        /// 写 N 个 90
        #[arg(long, conflicts_with_all = ["hex", "jmp", "set_u32"])]
        nops: Option<usize>,
        /// 写一条跳到该地址的跳转
        #[arg(long, value_parser = parse_int, conflicts_with_all = ["hex", "nops", "set_u32"])]
        jmp: Option<u32>,
        /// 写一个 u32（小端）
        #[arg(long, value_parser = parse_int, conflicts_with_all = ["hex", "nops", "jmp", "set_u32", "set_u16"])]
        set_u32: Option<u32>,
        /// 写一个 u16（小端）
        #[arg(long, conflicts_with_all = ["hex", "nops", "jmp", "set_u32"])]
        set_u16: Option<u16>,
        /// 写一个 u8
        #[arg(long, conflicts_with_all = ["hex", "nops", "jmp", "set_u32", "set_u16"])]
        set_u8: Option<u8>,
    },
    /// 按补丁表打补丁
    Apply {
        /// 输入 exe
        #[arg(long)]
        exe: PathBuf,
        /// 补丁表（YAML）
        #[arg(long)]
        patches: PathBuf,
        /// 输出 exe
        #[arg(long)]
        out: PathBuf,
        /// JSON 报告
        #[arg(long)]
        report: Option<PathBuf>,
        /// 只报告不写文件
        #[arg(long)]
        dry_run: bool,
        /// 允许部分失败（失败也写 out）
        #[arg(long)]
        allow_partial: bool,
    },
    /// 校验一个 exe 是否已经打好补丁
    Verify {
        /// 被校验的 exe
        #[arg(long)]
        exe: PathBuf,
        /// 补丁表（YAML）
        #[arg(long)]
        patches: PathBuf,
        /// 原始（未打补丁）exe —— 给了就能精确定位落点
        #[arg(long)]
        original: Option<PathBuf>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
}

/// 数字参数：`0x` 开头按十六进制，否则按十进制。
fn parse_int(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let (t, radix) = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(rest) => (rest, 16),
        None => (t, 10),
    };
    u32::from_str_radix(t, radix).map_err(|e| format!("{s} 不是合法数字：{e}"))
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("错误：{e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<i32> {
    match cli.cmd {
        Cmd::Info { exe } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let (pe, notes) = pe::analyze(&data)?;
            println!("文件      : {} ({} 字节)", exe.display(), data.len());
            println!("位数      : {}", if pe.is_64 { "PE32+" } else { "PE32" });
            println!("镜像基址  : 0x{:08X}", pe.image_base);
            println!("入口点    : 0x{:08X}", pe.entry_point);
            println!(
                "编译时间  : {} (0x{:08X}, YYYYMMDD={})",
                pe.build_date_str(),
                pe.build_date,
                pe.build_date_ymd()
            );
            println!("段表      :");
            for s in &pe.sections {
                println!(
                    "  {:<10} VA=0x{:08X} vsize=0x{:08X} raw=0x{:08X}+0x{:08X}",
                    s.name, s.va, s.vsize, s.raw_ptr, s.raw_size
                );
            }
            if notes.is_empty() {
                println!("壳检测    : 未发现常见加壳段");
            } else {
                for n in notes {
                    println!("壳检测    : {n}");
                }
            }
            Ok(0)
        }
        Cmd::Find {
            exe,
            hex,
            text,
            encoding,
            section,
            nth,
            last,
            limit,
        } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let eng = engine::Engine::new(data)?;
            let range = eng.range_of(section.as_deref())?;
            let hits = if let Some(h) = &hex {
                eng.find_hex(h, range)?
            } else if let Some(t) = &text {
                let enc = match &encoding {
                    Some(n) => Encoding::from_name(n)?,
                    None => Encoding::Gbk,
                };
                eng.find_text(&encode::encode(t, enc)?, range)
            } else {
                bail!("要给 --hex 或 --text 其中之一");
            };
            println!("{} 处命中", hits.len());
            let mut shown = 0;
            for h in hits.iter() {
                if shown >= limit {
                    break;
                }
                shown += 1;
                let va = eng.va_of_offset(*h);
                let sec = eng.pe.section_of_offset(*h).map(|s| s.name.clone());
                println!(
                    "  文件偏移 0x{:08X}  VA {}  段 {}",
                    h,
                    va.map(|v| format!("0x{v:08X}")).unwrap_or("-".into()),
                    sec.unwrap_or_else(|| "-".into()),
                );
            }
            // 第 N 次命中 / 最后一次命中（走引擎的 FindHexN / FindLastHex）
            if let (Some(n), Some(h)) = (nth, &hex) {
                match eng.find_hex_nth(h, range, n.max(1))? {
                    Some(off) => println!("第 {n} 次命中：文件偏移 0x{off:08X}"),
                    None => println!("第 {n} 次命中：不存在"),
                }
            }
            if last {
                match hex.as_ref().map(|h| eng.find_last_hex(h, range)) {
                    Some(Ok(Some(off))) => println!("最后一次命中：文件偏移 0x{off:08X}"),
                    Some(Ok(None)) => println!("最后一次命中：不存在"),
                    _ => println!("最后一次命中：{}",
                        hits.last().map(|h| format!("文件偏移 0x{h:08X}")).unwrap_or_else(|| "不存在".into())),
                }
            }
            Ok(0)
        }
        Cmd::Read {
            exe,
            va,
            len,
            u8,
            u16,
            u32,
            tgt,
        } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let eng = engine::Engine::new(data)?;
            if u8 {
                println!("U8  @0x{va:08X} = 0x{:02X} ({})", eng.read_u8_va(va)?, eng.read_u8_va(va)?);
            }
            if u16 {
                let v = eng.read_u16_va(va)?;
                println!("U16 @0x{va:08X} = 0x{v:04X} ({v})");
            }
            if u32 {
                let v = eng.read_u32_va(va)?;
                println!("U32 @0x{va:08X} = 0x{v:08X} ({v})");
            }
            if !u8 && !u16 && !u32 {
                let off = eng.offset_of_va(va)?;
                let end = (off + len).min(eng.data.len());
                let bytes = &eng.data[off..end];
                println!(
                    "0x{va:08X} (文件偏移 0x{off:08X}) = {}",
                    hex::format_bytes(bytes)
                );
            }
            if tgt {
                let t = eng.get_tgt_addr(va)?;
                println!("跳转目标 = 0x{t:08X}");
            }
            Ok(0)
        }
        Cmd::Write {
            exe,
            out,
            va,
            hex: hexstr,
            nops,
            jmp,
            set_u32,
            set_u16,
            set_u8,
        } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let mut eng = engine::Engine::new(data)?;
            let what = if let Some(h) = &hexstr {
                let bytes = hex::parse_bytes(h)?;
                eng.write_hex_va(va, &bytes)?;
                format!("写 {} 字节：{}", bytes.len(), hex::format_bytes(&bytes))
            } else if let Some(n) = nops {
                eng.write_nops_va(va, n)?;
                format!("写 {n} 个 NOP")
            } else if let Some(t) = jmp {
                let kind = eng.write_jmp_va(va, t)?;
                match kind {
                    engine::JmpKind::Rel32 => format!("写 E9 rel32（{} 字节）→ 0x{t:08X}", kind.len()),
                    engine::JmpKind::Rel8 => format!("写 EB rel8（{} 字节）→ 0x{t:08X}", kind.len()),
                }
            } else if let Some(v) = set_u32 {
                eng.write_u32_va(va, v)?;
                format!("写 U32 = 0x{v:08X}")
            } else if let Some(v) = set_u16 {
                eng.write_u16_va(va, v)?;
                format!("写 U16 = 0x{v:04X}")
            } else if let Some(v) = set_u8 {
                eng.write_u8_va(va, v)?;
                format!("写 U8 = 0x{v:02X}")
            } else {
                bail!("要给 --hex / --nops / --jmp / --set-u32 其中之一");
            };
            println!("{what} @0x{va:08X}");
            std::fs::write(&out, &eng.data).with_context(|| format!("写 {} 失败", out.display()))?;
            println!("输出 exe  : {}", out.display());
            Ok(0)
        }
        Cmd::Apply {
            exe,
            patches,
            out,
            report,
            dry_run,
            allow_partial,
        } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let tbl = table::PatchTable::load(&patches)?;
            let opts = ApplyOptions {
                out: Some(out.clone()),
                dry_run,
                allow_partial,
            };
            let (mut rep, out_bytes) = apply::apply(data, &tbl, &opts)?;
            rep.exe = exe.display().to_string();
            rep.patches = patches.display().to_string();
            rep.out = if dry_run {
                None
            } else {
                Some(out.display().to_string())
            };

            // 打印摘要
            println!("补丁表    : {}", patches.display());
            println!("输入 exe  : {}", exe.display());
            println!(
                "编译时间  : {}",
                rep.build_date
            );
            println!(
                "合计 {} 条：打上 {} / 已存在 {} / 跳过 {} / 失败 {}",
                rep.total, rep.applied, rep.already, rep.skipped, rep.failed
            );
            for e in &rep.entries {
                let mark = match e.status.as_str() {
                    "applied" => "✔",
                    "already" => "=",
                    "skipped" => "-",
                    _ => "✘",
                };
                let loc = match (e.offset, e.va) {
                    (Some(o), Some(v)) => format!("偏移 0x{o:08X} / VA 0x{v:08X}",),
                    (Some(o), None) => format!("偏移 0x{o:08X}",),
                    _ => String::new(),
                };
                let bytes = match (&e.old, &e.new) {
                    (Some(o), Some(n)) => format!("  {} → {}", o, n),
                    _ => String::new(),
                };
                println!("  {mark} {:<28} {:<9} {loc}{bytes}", e.id, e.status);
                if let Some(r) = &e.reason {
                    println!("      ↳ {r}");
                }
            }

            // 写 out
            if dry_run {
                println!("dry-run：没有写任何文件");
            } else if let Some(bytes) = &out_bytes {
                std::fs::write(&out, bytes)
                    .with_context(|| format!("写 {} 失败", out.display()))?;
                println!("输出 exe  : {}", out.display());
            } else {
                println!(
                    "有补丁失败，整份补丁没有落盘（要强制输出请加 --allow-partial）"
                );
            }

            // 写报告
            if let Some(rp) = &report {
                let json = serde_json::to_string_pretty(&rep)?;
                std::fs::write(rp, json).with_context(|| format!("写 {} 失败", rp.display()))?;
                println!("JSON 报告 : {}", rp.display());
            }

            Ok(if rep.ok { 0 } else { 1 })
        }
        Cmd::Verify {
            exe,
            patches,
            original,
            json,
        } => {
            let data = std::fs::read(&exe).with_context(|| format!("读 {} 失败", exe.display()))?;
            let tbl = table::PatchTable::load(&patches)?;
            let orig = match &original {
                Some(p) => Some(
                    std::fs::read(p).with_context(|| format!("读 {} 失败", p.display()))?,
                ),
                None => None,
            };
            let res = apply::verify(data, &tbl, orig.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!(
                    "校验 {}：通过 {} / 跳过 {} / 失败 {}",
                    exe.display(),
                    res.passed,
                    res.skipped,
                    res.failed
                );
                for e in &res.entries {
                    let mark = match e.status.as_str() {
                        "ok" => "✔",
                        "skipped" => "-",
                        _ => "✘",
                    };
                    println!(
                        "  {mark} {:<28} {:<8} {}",
                        e.id,
                        e.status,
                        e.reason.clone().unwrap_or_default()
                    );
                }
            }
            Ok(if res.ok { 0 } else { 1 })
        }
    }
}
