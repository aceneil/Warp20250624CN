//! 补丁表的应用 / 校验，以及可核验的 JSON 报告。
//!
//! 语义（和 WARP 对齐，但更严格）：
//!
//! * `find` 在 `section` 限定的 raw 范围里搜索（没限定就是整文件）。
//! * `expect` 是**期望命中次数**，不符即失败（整份补丁不落盘，除非 `--allow-partial`）。
//! * 替换是**原地覆写**：替换字节数必须 ≤ find 字节数，超了直接跳过这条。
//! * 幂等：目标位置已经是替换后的字节 → 状态 `already`，不重复写。
//! * 编译时间不在 `dates` 区间内的条目 → 状态 `skipped`，不算失败。

use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

use crate::encode::{self, Encoding};
use crate::engine::Engine;
use crate::hex::{self, PatByte};
use crate::table::{Mode, PatchSpec, PatchTable};

/// 单条补丁的处理结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 已经打上了（本次写入）。
    Applied,
    /// 目标位置已经是替换后的字节（幂等）。
    Already,
    /// 跳过（禁用 / 日期不匹配 / 和别的补丁重叠）。
    Skipped,
    /// 失败。
    Fail,
}

impl Status {
    pub fn name(&self) -> &'static str {
        match self {
            Status::Applied => "applied",
            Status::Already => "already",
            Status::Skipped => "skipped",
            Status::Fail => "fail",
        }
    }
}

/// 报告里的一个落点。
#[derive(Debug, Clone, Serialize)]
pub struct Location {
    /// 文件偏移。
    pub offset: usize,
    /// 文件偏移（十六进制，方便人看）。
    pub offset_hex: String,
    /// 虚拟地址（偏移不在任何段里时为 null）。
    pub va: Option<u32>,
    /// 虚拟地址（十六进制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub va_hex: Option<String>,
    /// 落点所属的段。
    pub section: Option<String>,
    /// 旧字节。
    pub old: String,
    /// 新字节。
    pub new: String,
    /// 这个落点的状态。
    pub status: String,
}

/// 报告里的一条补丁。
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// 补丁 id。
    pub id: String,
    /// 中文说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// 状态：applied / already / skipped / fail。
    pub status: String,
    /// 失败或跳过的原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 补丁模式。
    pub mode: String,
    /// 限定的段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// 期望命中次数。
    pub expect: usize,
    /// 实际命中次数。
    pub hits: usize,
    /// 第一个落点的文件偏移。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 第一个落点的文件偏移（十六进制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_hex: Option<String>,
    /// 第一个落点的 VA。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub va: Option<u32>,
    /// 第一个落点的 VA（十六进制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub va_hex: Option<String>,
    /// 第一个落点的旧字节。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    /// 第一个落点的新字节。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
    /// 全部落点。
    pub locations: Vec<Location>,
}

/// 整份补丁的应用报告。
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// 工具名。
    pub tool: String,
    /// 输入 exe。
    pub exe: String,
    /// 输出 exe（dry-run 时为 null）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    /// 补丁表路径。
    pub patches: String,
    /// PE 编译时间。
    pub build_date: String,
    /// 是否 dry-run。
    pub dry_run: bool,
    /// 总条数。
    pub total: usize,
    /// 打上多少条。
    pub applied: usize,
    /// 已经打过多少条。
    pub already: usize,
    /// 跳过多少条。
    pub skipped: usize,
    /// 失败多少条。
    pub failed: usize,
    /// 是否全部通过。
    pub ok: bool,
    /// 每条补丁的明细。
    pub entries: Vec<Entry>,
}

/// `apply` 的参数。
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// 输出文件。
    pub out: Option<PathBuf>,
    /// 只报告不写文件。
    pub dry_run: bool,
    /// 允许部分失败（失败时也写 out）。
    pub allow_partial: bool,
}

/// 解析后的一条补丁（内部用）。
struct Resolved {
    spec: PatchSpec,
    mode: Mode,
    /// 搜索模式。
    find: Vec<PatByte>,
    /// 替换字节（不含结束符）。
    repl: Vec<u8>,
    /// 替换后是否补 `00`。
    terminate: bool,
    /// 搜索范围。
    range: (usize, usize),
}

/// 一条补丁的落点。
struct Target {
    offset: usize,
    va: Option<u32>,
    /// 落点上原来的字节（写之前抓的）。
    old: Vec<u8>,
    new: Vec<u8>,
}

/// 应用补丁表。
pub fn apply(
    data: Vec<u8>,
    table: &PatchTable,
    opts: &ApplyOptions,
) -> Result<(Report, Option<Vec<u8>>)> {
    let exe_len = data.len();
    let mut engine = Engine::new(data)?;
    let build_date = engine.pe.build_date_str();

    // ---------------- 第一阶段：解析每条补丁 ----------------
    let mut resolved: Vec<Result<Resolved>> = Vec::with_capacity(table.patches.len());
    for spec in &table.patches {
        resolved.push(resolve(&engine, spec));
    }

    // ---------------- 第二阶段：定位所有落点 ----------------
    struct Plan {
        status: Status,
        reason: Option<String>,
        hits: usize,
        targets: Vec<Target>,
        /// 已经打好的落点（幂等判断用）。
        already_at: Option<usize>,
    }

    let mut plans: Vec<Plan> = Vec::with_capacity(table.patches.len());
    for (_idx, (_spec, res)) in table.patches.iter().zip(resolved.into_iter()).enumerate() {
        let mut plan = Plan {
            status: Status::Skipped,
            reason: None,
            hits: 0,
            targets: Vec::new(),
            already_at: None,
        };

        let r = match res {
            Ok(r) => r,
            Err(e) => {
                plan.status = Status::Fail;
                plan.reason = Some(format!("{e:#}"));
                plans.push(plan);
                continue;
            }
        };

        // 禁用
        if r.spec.disabled {
            plan.reason = Some(
                r.spec
                    .note
                    .clone()
                    .unwrap_or_else(|| "该补丁被标记为不启用".to_string()),
            );
            plans.push(plan);
            continue;
        }

        // 编译时间过滤
        if !date_in_range(r.spec.dates.as_ref(), engine.pe.build_date_ymd()) {
            plan.reason = Some(format!(
                "编译时间 {} 不在日期区间内，跳过",
                engine.pe.build_date_str()
            ));
            plans.push(plan);
            continue;
        }

        // 找
        let hits = engine.find_pattern(&r.find, r.range);
        plan.hits = hits.len();

        // 选落点
        let selected: Vec<usize> = if r.spec.all {
            hits.clone()
        } else if r.spec.last {
            hits.last().copied().into_iter().collect()
        } else {
            let nth = r.spec.nth.unwrap_or(1).max(1);
            hits.get(nth - 1).copied().into_iter().collect()
        };

        if selected.is_empty() {
            // 找不到 → 幂等兜底：看看替换后的字节是不是已经在文件里了
            if let Some((off, bytes)) = already_fallback(&engine, &r) {
                plan.targets.push(Target {
                    offset: off,
                    va: engine.va_of_offset(off),
                    old: engine.data[off..(off + bytes.len()).min(engine.data.len())].to_vec(),
                    new: bytes,
                });
                plan.status = Status::Already;
                plan.already_at = Some(off);
                plan.reason = Some("未找到原始特征串，但替换后的字节已存在（已打过补丁）".into());
            } else {
                plan.status = Status::Fail;
                plan.reason = Some("未找到 find 模式".into());
            }
            plans.push(plan);
            continue;
        }

        // 算每个落点要写什么
        let mut build_err: Option<String> = None;
        for off in &selected {
            match compute_target(&engine, &r, *off) {
                Ok(t) => plan.targets.push(t),
                Err(e) => {
                    build_err = Some(format!("{e:#}"));
                    break;
                }
            }
        }
        if let Some(e) = build_err {
            // 落点算不出来（比如要补的结束符会盖掉非 0x00 的字节）：
            // 先当成「已经打过补丁」试试，还不行才算失败
            if let Some((off, bytes)) = already_fallback(&engine, &r) {
                plan.targets.push(Target {
                    offset: off,
                    va: engine.va_of_offset(off),
                    old: engine.data[off..(off + bytes.len()).min(engine.data.len())].to_vec(),
                    new: bytes,
                });
                plan.status = Status::Already;
                plan.already_at = Some(off);
                plan.reason = Some(format!("{e}（但替换后的字节已存在，判定为已打过补丁）"));
            } else {
                plan.status = Status::Fail;
                plan.reason = Some(e);
            }
            plans.push(plan);
            continue;
        }

        // 幂等判断：落点上的字节已经等于要写的内容？
        let already_all = plan.targets.iter().all(|t| {
            engine.data[t.offset..t.offset + t.new.len()] == t.new[..]
        });
        if already_all {
            plan.status = Status::Already;
            plan.already_at = Some(plan.targets[0].offset);
            // 命中次数对不上也要说清楚（可能是别人改过表）
            if r.spec.expect.is_some() && plan.hits != r.spec.expect() {
                plan.reason = Some(format!(
                    "命中 {} 次（期望 {} 次），但目标位置已经是替换后的字节",
                    plan.hits,
                    r.spec.expect()
                ));
            }
            plans.push(plan);
            continue;
        }

        // 命中次数校验
        if plan.hits != r.spec.expect() {
            // 命中数对不上，多半是「已经打过补丁了」（特征串被换掉了）→ 幂等兜底
            if let Some((off, bytes)) = already_fallback(&engine, &r) {
                plan.targets.clear();
                plan.targets.push(Target {
                    offset: off,
                    va: engine.va_of_offset(off),
                    old: engine.data[off..(off + bytes.len()).min(engine.data.len())].to_vec(),
                    new: bytes,
                });
                plan.status = Status::Already;
                plan.already_at = Some(off);
                plan.reason = Some(format!(
                    "命中 {} 次（期望 {} 次），但替换后的字节已存在（已打过补丁）",
                    plan.hits,
                    r.spec.expect()
                ));
            } else {
                plan.status = Status::Fail;
                plan.reason = Some(format!(
                    "命中 {} 次，期望 {} 次",
                    plan.hits,
                    r.spec.expect()
                ));
            }
            plans.push(plan);
            continue;
        }

        plan.status = Status::Applied;
        plans.push(plan);
    }

    // ---------------- 第三阶段：冲突检测（两条补丁写同一块） ----------------
    // 按优先级大 → 表格顺序，小的先打；重叠的后者跳过。
    let mut order: Vec<usize> = (0..plans.len()).collect();
    order.sort_by_key(|&i| {
        (
            -table.patches[i].priority(),
            i,
        )
    });

    let mut written: Vec<(usize, usize, usize)> = Vec::new(); // (起, 止, 补丁序号)
    for &i in &order {
        if plans[i].status != Status::Applied {
            continue;
        }
        let mut conflict_with: Option<String> = None;
        for t in &plans[i].targets {
            let (s, e) = (t.offset, t.offset + t.new.len());
            for (ws, we, owner) in &written {
                if s < *we && *ws < e {
                    conflict_with = Some(table.patches[*owner].id.clone());
                    break;
                }
            }
            if conflict_with.is_some() {
                break;
            }
        }
        if let Some(other) = conflict_with {
            plans[i].status = Status::Skipped;
            plans[i].reason = Some(format!("写入区域与补丁 {other} 重叠，跳过"));
            continue;
        }
        for t in &plans[i].targets {
            written.push((t.offset, t.offset + t.new.len(), i));
        }
    }

    // ---------------- 第四阶段：真正写 ----------------
    for &i in &order {
        if plans[i].status != Status::Applied {
            continue;
        }
        let mut write_err: Option<String> = None;
        for t in &mut plans[i].targets {
            // 先把旧字节抓下来（报告里要用），再写
            t.old = engine.data[t.offset..t.offset + t.new.len()].to_vec();
            if opts.dry_run {
                continue;
            }
            if let Err(e) = engine.write_hex_offset(t.offset, &t.new) {
                write_err = Some(format!("{e:#}"));
                break;
            }
        }
        if let Some(e) = write_err {
            plans[i].status = Status::Fail;
            plans[i].reason = Some(e);
        }
    }

    // ---------------- 汇总报告 ----------------
    let mut entries = Vec::with_capacity(table.patches.len());
    let (mut n_applied, mut n_already, mut n_skipped, mut n_failed) = (0, 0, 0, 0);
    for (i, spec) in table.patches.iter().enumerate() {
        let plan = &plans[i];
        let mut locations = Vec::new();
        let mut first: Option<(usize, Option<u32>, String, String)> = None;

        if plan.status == Status::Applied {
            for t in &plan.targets {
                let old = hex::format_bytes(&t.old);
                locations.push(Location {
                    offset: t.offset,
                    offset_hex: format!("0x{:08X}", t.offset),
                    va: t.va,
                    va_hex: t.va.map(|v| format!("0x{v:08X}")),
                    section: engine
                        .pe
                        .section_of_offset(t.offset)
                        .map(|s| s.name.clone()),
                    old,
                    new: hex::format_bytes(&t.new),
                    status: Status::Applied.name().to_string(),
                });
            }
            if let Some(t) = plan.targets.first() {
                first = Some((
                    t.offset,
                    t.va,
                    hex::format_bytes(&t.old),
                    hex::format_bytes(&t.new),
                ));
            }
            n_applied += 1;
        } else if plan.status == Status::Already {
            let off = plan.already_at.unwrap_or(0);
            let len = plan
                .targets
                .first()
                .map(|t| t.new.len())
                .unwrap_or(1);
            let old = hex::format_bytes(&engine.data[off..(off + len).min(engine.data.len())]);
            locations.push(Location {
                offset: off,
                offset_hex: format!("0x{off:08X}"),
                va: engine.va_of_offset(off),
                va_hex: engine.va_of_offset(off).map(|v| format!("0x{v:08X}")),
                section: engine.pe.section_of_offset(off).map(|s| s.name.clone()),
                old: old.clone(),
                new: hex::format_bytes(
                    &plan.targets.first().map(|t| t.new.clone()).unwrap_or_default(),
                ),
                status: Status::Already.name().to_string(),
            });
            first = Some((
                off,
                engine.va_of_offset(off),
                old.clone(),
                hex::format_bytes(
                    &plan.targets.first().map(|t| t.new.clone()).unwrap_or_default(),
                ),
            ));
            n_already += 1;
        } else if plan.status == Status::Skipped {
            n_skipped += 1;
        } else {
            n_failed += 1;
        }

        entries.push(Entry {
            id: spec.id.clone(),
            desc: spec.desc.clone(),
            status: plan.status.name().to_string(),
            reason: plan.reason.clone(),
            mode: spec.mode().map(|m| m.name().to_string()).unwrap_or_else(|_| "replace".into()),
            section: spec.section.clone(),
            expect: spec.expect(),
            hits: plan.hits,
            offset: first.as_ref().map(|f| f.0),
            offset_hex: first.as_ref().map(|f| format!("0x{:08X}", f.0)),
            va: first.as_ref().and_then(|f| f.1),
            va_hex: first.as_ref().and_then(|f| f.1).map(|v| format!("0x{v:08X}")),
            old: first.as_ref().map(|f| f.2.clone()),
            new: first.as_ref().map(|f| f.3.clone()),
            locations,
        });
    }

    let ok = n_failed == 0;
    let report = Report {
        tool: "warp_rs".to_string(),
        exe: String::new(),
        out: opts.out.as_ref().map(|p| p.display().to_string()),
        patches: String::new(),
        build_date,
        dry_run: opts.dry_run,
        total: table.patches.len(),
        applied: n_applied,
        already: n_already,
        skipped: n_skipped,
        failed: n_failed,
        ok,
        entries,
    };

    // ---------------- 落盘 ----------------
    let out_bytes = if ok || opts.allow_partial {
        if opts.dry_run {
            None
        } else {
            Some(engine.data.clone())
        }
    } else {
        None
    };

    let _ = exe_len;
    Ok((report, out_bytes))
}

/// 解析一条补丁（找模式、替换内容、搜索范围）。
fn resolve(engine: &Engine, spec: &PatchSpec) -> Result<Resolved> {
    let mode = spec.mode()?;
    let find = hex::parse_pattern(&spec.find)
        .with_context(|| format!("find 模式解析失败：{}", spec.find))?;
    let range = engine.range_of(spec.section.as_deref())?;

    let mut repl: Vec<u8> = Vec::new();
    let mut terminate = spec.terminate.unwrap_or(false);
    match mode {
        Mode::Replace => {
            if let Some(hexstr) = &spec.replace {
                repl = hex::parse_bytes(hexstr)
                    .with_context(|| format!("replace 字节串解析失败：{hexstr}"))?;
            } else if let Some(text) = &spec.text {
                let enc = match &spec.encoding {
                    Some(name) => Encoding::from_name(name)?,
                    None => Encoding::Gbk,
                };
                repl = encode::encode(text, enc)?;
                // 文本补丁默认补字符串结束符（和 WARP 的 SetText 一致）
                if spec.terminate.is_none() {
                    terminate = true;
                }
            } else {
                bail!("补丁 {} 既没有 replace 也没有 text", spec.id);
            }
        }
        Mode::Nop => {
            let len = spec.nop_len.unwrap_or(find.len());
            repl = vec![0x90u8; len];
            terminate = false;
        }
        Mode::Jmp => {
            if spec.target.is_none() {
                bail!("补丁 {} 是 jmp 模式，但没有 target", spec.id);
            }
            terminate = false;
        }
    }

    Ok(Resolved {
        spec: spec.clone(),
        mode,
        find,
        repl,
        terminate,
        range,
    })
}

/// 算一个落点要写什么字节，并做长度 / 边界校验。
fn compute_target(engine: &Engine, r: &Resolved, off: usize) -> Result<Target> {
    let va = engine.va_of_offset(off);
    let new = match r.mode {
        Mode::Replace => {
            // 铁律：替换字节数 ≤ find 字节数（原地覆写）；
            // 例外：允许借用紧跟在匹配之后的连续 0x00 填充 —— 只覆盖 NUL，
            // 不碰任何非零数据，也不会跨段。
            if r.repl.len() > r.find.len() {
                let mut pad = 0usize;
                let mut i = off + r.find.len();
                while i < engine.data.len() && engine.data[i] == 0x00 {
                    pad += 1;
                    i += 1;
                }
                ensure!(
                    r.repl.len() <= r.find.len() + pad,
                    "替换字节数 {} 超过 find 的 {} 字节（尾部可用 NUL 填充 {} 字节），跳过",
                    r.repl.len(),
                    r.find.len(),
                    pad
                );
                // 借用填充时，落地范围必须仍在同一段内
                engine.check_write(off, r.repl.len())?;
            }
            let mut buf = r.repl.clone();
            if r.terminate {
                buf.push(0x00);
                if buf.len() > r.find.len() {
                    // 允许超出 1 字节，但那一字节必须本来就是字符串结束符
                    let tail = off + r.find.len();
                    ensure!(
                        tail < engine.data.len() && engine.data[tail] == 0x00,
                        "替换后需要补结束符，但会覆盖非 0x00 的字节（偏移 0x{tail:X}），跳过"
                    );
                }
            }
            buf
        }
        Mode::Nop => r.repl.clone(),
        Mode::Jmp => {
            let target = r.spec.target.unwrap() as u32;
            let from = va.context("落点不在任何段里，无法计算跳转")?;
            let rel = target as i64 - (from as i64 + 5);
            ensure!(
                (-2147483648..=2147483647).contains(&rel),
                "跳转距离 {rel} 超出 rel32 范围"
            );
            let mut buf = vec![0xE9u8];
            buf.extend_from_slice(&(rel as i32).to_le_bytes());
            buf
        }
    };
    // 写入边界检查
    engine.check_write(off, new.len())?;
    Ok(Target {
        offset: off,
        va,
        old: Vec::new(),
        new,
    })
}

/// 一条补丁实际写进文件的字节（替换内容 + 可选的字符串结束符）。
fn expected_bytes(r: &Resolved) -> Vec<u8> {
    let mut b = r.repl.clone();
    if r.terminate {
        b.push(0x00);
    }
    b
}

/// 幂等兜底：在范围里找替换后的字节，找到就返回 (偏移, 应有的字节)。
fn already_fallback(engine: &Engine, r: &Resolved) -> Option<(usize, Vec<u8>)> {
    let off = find_replacement(engine, &r.repl, r.terminate, r.range)?;
    Some((off, expected_bytes(r)))
}

/// 幂等兜底：在范围里找替换后的字节。
fn find_replacement(
    engine: &Engine,
    repl: &[u8],
    terminate: bool,
    range: (usize, usize),
) -> Option<usize> {
    if repl.is_empty() {
        return None;
    }
    let mut needle = repl.to_vec();
    if terminate {
        needle.push(0x00);
    }
    engine.find_text(&needle, range).into_iter().next()
}

/// 编译时间是否落在区间内。
fn date_in_range(
    range: Option<&crate::table::DateRange>,
    build_ymd: u32,
) -> bool {
    let Some(r) = range else { return true };
    let norm = |v: u32| -> u32 {
        // 大于 1e8 的当成 unix 时间戳，换算成 YYYYMMDD 再比
        if v > 100_000_000 {
            let days = (v as u64 / 86400) as i64;
            let (y, m, d) = crate::pe::civil_from_days_pub(days);
            (y as u32) * 10000 + m * 100 + d
        } else {
            v
        }
    };
    if let Some(start) = r.start {
        if build_ymd < norm(start) {
            return false;
        }
    }
    if let Some(end) = r.end {
        if build_ymd > norm(end) {
            return false;
        }
    }
    true
}

/// 校验一个 exe 是否已经打好补丁。
#[derive(Debug, Clone, Serialize)]
pub struct VerifyResult {
    pub ok: bool,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub entries: Vec<VerifyEntry>,
}

/// 校验结果里的一条。
#[derive(Debug, Clone, Serialize)]
pub struct VerifyEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub va: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
}

/// 校验：每条补丁的替换内容是否已经在文件里。
///
/// 给了 `original` 时用原始文件定位落点（最准）；没给就退化成
/// 「替换字节是否出现在搜索范围里」。
pub fn verify(data: Vec<u8>, table: &PatchTable, original: Option<&[u8]>) -> Result<VerifyResult> {
    let engine = Engine::new(data)?;
    let orig_engine = match original {
        Some(o) => Some(Engine::new(o.to_vec())?),
        None => None,
    };

    let mut entries = Vec::new();
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);

    for spec in &table.patches {
        let mut entry = VerifyEntry {
            id: spec.id.clone(),
            desc: spec.desc.clone(),
            status: "fail".into(),
            reason: None,
            offset: None,
            va: None,
            expected: None,
            actual: None,
        };

        if spec.disabled {
            entry.status = "skipped".into();
            entry.reason = Some(spec.note.clone().unwrap_or_else(|| "已禁用".into()));
            skipped += 1;
            entries.push(entry);
            continue;
        }
        if !date_in_range(spec.dates.as_ref(), engine.pe.build_date_ymd()) {
            entry.status = "skipped".into();
            entry.reason = Some("编译时间不在日期区间内".into());
            skipped += 1;
            entries.push(entry);
            continue;
        }

        let mode = spec.mode().unwrap_or(Mode::Replace);
        let repl: Vec<u8> = if mode == Mode::Nop {
            vec![0x90u8; spec.nop_len.unwrap_or_else(|| hex::parse_pattern(&spec.find).map(|p| p.len()).unwrap_or(1))]
        } else if let Some(h) = &spec.replace {
            hex::parse_bytes(h).unwrap_or_default()
        } else if let Some(t) = &spec.text {
            let enc = match &spec.encoding {
                Some(n) => Encoding::from_name(n).unwrap_or(Encoding::Gbk),
                None => Encoding::Gbk,
            };
            encode::encode(t, enc).unwrap_or_default()
        } else {
            Vec::new()
        };
        let terminate = spec.terminate.unwrap_or(spec.text.is_some() && mode == Mode::Replace);
        let mut expected = repl.clone();
        if terminate {
            expected.push(0x00);
        }

        let range = match engine.range_of(spec.section.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                entry.reason = Some(format!("{e:#}"));
                failed += 1;
                entries.push(entry);
                continue;
            }
        };

        // 有原始文件 → 按原始文件定位落点，逐字节比对
        if let Some(orig) = &orig_engine {
            let orig_range = match orig.range_of(spec.section.as_deref()) {
                Ok(r) => r,
                Err(_) => (0, orig.data.len()),
            };
            let hits = orig.find_pattern(&hex::parse_pattern(&spec.find)?, orig_range);
            let selected: Vec<usize> = if spec.all {
                hits.clone()
            } else if spec.last {
                hits.last().copied().into_iter().collect()
            } else {
                let nth = spec.nth.unwrap_or(1).max(1);
                hits.get(nth - 1).copied().into_iter().collect()
            };
            if selected.is_empty() {
                entry.status = "fail".into();
                entry.reason = Some("原始文件里找不到 find 模式".into());
                failed += 1;
            } else {
                let off = selected[0];
                let actual = engine.data.get(off..off + expected.len()).unwrap_or(&[]).to_vec();
                entry.offset = Some(off);
                entry.va = engine.va_of_offset(off);
                entry.expected = Some(hex::format_bytes(&expected));
                entry.actual = Some(hex::format_bytes(&actual));
                if actual == expected {
                    entry.status = "ok".into();
                    passed += 1;
                } else {
                    entry.status = "fail".into();
                    entry.reason = Some("落点字节和期望不一致".into());
                    failed += 1;
                }
            }
            entries.push(entry);
            continue;
        }

        // 没有原始文件 → 看替换内容是否已经出现在范围里
        let found = if expected.is_empty() {
            None
        } else {
            engine.find_text(&expected, range).into_iter().next()
        };
        match found {
            Some(off) => {
                entry.status = "ok".into();
                entry.offset = Some(off);
                entry.va = engine.va_of_offset(off);
                entry.expected = Some(hex::format_bytes(&expected));
                entry.actual = Some(hex::format_bytes(
                    &engine.data[off..(off + expected.len()).min(engine.data.len())],
                ));
                passed += 1;
            }
            None => {
                entry.status = "fail".into();
                entry.reason = Some("替换后的内容没有出现在文件里".into());
                failed += 1;
            }
        }
        entries.push(entry);
    }

    Ok(VerifyResult {
        ok: failed == 0,
        total: table.patches.len(),
        passed,
        failed,
        skipped,
        entries,
    })
}
