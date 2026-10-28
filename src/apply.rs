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
use crate::params::{self, ParamRecord, ParamResolver, ParamValues, Source};
use crate::patches::{self, Ctx, Outcome};
use crate::table::{Mode, PatchSpec, PatchTable, TableMeta};

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
    /// 补充说明（指令上下文、imm32 旧值 → 新值……）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 报告里的一条补丁。
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// 补丁 id。
    pub id: String,
    /// 中文说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// 英文原文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc_en: Option<String>,
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
    /// 这条补丁实际用的写入编码。
    pub encoding: String,
    /// 编码的来源：cli / ask / spec / meta / default。
    pub encoding_source: String,
    /// 这条补丁用到的参数（值 + 来源）。
    pub params: Vec<ParamRecord>,
    /// 这条补丁的特征串在这个客户端里找不到（版本不符？）。
    pub not_applicable: bool,
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
    /// 实际使用的写入编码。
    pub encoding: String,
    /// 本次用到的所有参数（值 + 来源）。
    pub params: Vec<ParamRecord>,
    /// 多少条补丁在这个客户端里找不到特征（不适用）。
    pub not_applicable: usize,
    /// 备注 / 提示。
    pub notes: Vec<String>,
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
    /// 参数来源（`--set` / `--interactive`）。
    pub params: ParamResolver,
    /// 命令行指定的写入编码（优先级最高）。
    pub encoding: Option<Encoding>,
}

/// 解析后的一条补丁（内部用）。
struct Resolved {
    spec: PatchSpec,
    mode: Mode,
    /// 搜索模式。
    find: Vec<PatByte>,
    /// 替换字节（不含结束符）。
    repl: Vec<u8>,
    /// 替换字节串（允许 `??` = 保留原字节）。
    repl_pat: Vec<Option<u8>>,
    /// 替换后是否补 `00`。
    terminate: bool,
    /// 替换字节比 find 短时，把 find 剩下的字节清零。
    zero_fill: bool,
    /// 搜索范围。
    range: (usize, usize),
    /// 这条补丁实际用的写入编码。
    encoding: Encoding,
    /// 编码的来源：cli / ask / spec / meta / default。
    encoding_source: Source,
    /// 这条补丁用到的参数。
    params: Vec<ParamRecord>,
    /// 参数文本编码后的字节数（key + 字节数），用于「太长」报错。
    param_bytes: Option<(String, usize)>,
    /// 这条补丁用到的每个参数的字节数。
    param_bytes_all: Vec<(String, usize)>,
    /// 所有 ask 参数的值（key → 明文），自定义处理器用。
    param_values: ParamValues,
}

/// 一条补丁的落点。
struct Target {
    offset: usize,
    va: Option<u32>,
    /// 落点上原来的字节（写之前抓的）。
    old: Vec<u8>,
    new: Vec<u8>,
    /// 补充说明（报告里显示）。
    note: Option<String>,
}

/// 应用补丁表。
pub fn apply(
    data: Vec<u8>,
    table: &PatchTable,
    opts: &mut ApplyOptions,
) -> Result<(Report, Option<Vec<u8>>)> {
    let exe_len = data.len();
    let mut engine = Engine::new(data)?;
    let build_date = engine.pe.build_date_str();

    // 表级编码（命令行 > 表头 meta > 默认 gbk）；meta 写错了要当场报错
    let table_enc = table_encoding(table, opts.encoding)?;

    // ---------------- 第零阶段：解析参数（ask / --set / --interactive） ----------------
    let param_values = resolve_params(table, &mut opts.params)?;
    warn_unused_sets(table, &opts.params);

    // ---------------- 第一阶段：解析每条补丁 ----------------
    let mut resolved: Vec<Result<Resolved>> = Vec::with_capacity(table.patches.len());
    for spec in &table.patches {
        resolved.push(resolve_patch(
            &engine,
            spec,
            table.meta.as_ref(),
            opts.encoding,
            &param_values,
            &opts.params,
        ));
    }

    // 每条补丁用的编码 / 参数（报告要用）
    let mut encodings: Vec<String> = Vec::with_capacity(table.patches.len());
    let mut enc_sources: Vec<String> = Vec::with_capacity(table.patches.len());
    let mut patch_params: Vec<Vec<ParamRecord>> = Vec::with_capacity(table.patches.len());
    for res in resolved.iter() {
        match res {
            Ok(r) => {
                encodings.push(r.encoding.name().to_string());
                enc_sources.push(r.encoding_source.name().to_string());
                patch_params.push(r.params.clone());
            }
            Err(_) => {
                encodings.push(table_enc.name().to_string());
                enc_sources.push(Source::Default.name().to_string());
                patch_params.push(Vec::new());
            }
        }
    }
    // 把参数编码后的字节数记进参数表（报告里能看到「这个值占了几个字节」）
    for res in resolved.iter() {
        if let Ok(r) = res {
            for (k, n) in &r.param_bytes_all {
                opts.params.set_bytes(k, *n);
            }
        }
    }
    let param_records = opts.params.records();

    // ---------------- 第二阶段：定位所有落点 ----------------
    struct Plan {
        status: Status,
        reason: Option<String>,
        hits: usize,
        targets: Vec<Target>,
        /// 已经打好的落点（幂等判断用）。
        already_at: Option<usize>,
        /// 自定义补丁的处理器名。
        handler: Option<String>,
        /// 特征串在这个客户端里找不到（版本不符？）。
        not_applicable: bool,
        /// 这条补丁实际用的写入编码。
        enc: Encoding,
        /// 这条补丁的 ask 参数值。
        vals: ParamValues,
    }

    let mut plans: Vec<Plan> = Vec::with_capacity(table.patches.len());
    for (_idx, (_spec, res)) in table.patches.iter().zip(resolved.into_iter()).enumerate() {
        let mut plan = Plan {
            status: Status::Skipped,
            reason: None,
            hits: 0,
            targets: Vec::new(),
            already_at: None,
            handler: None,
            not_applicable: false,
            enc: table_enc,
            vals: ParamValues::new(),
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
        plan.enc = r.encoding;
        plan.vals = r.param_values.clone();

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

        // Rust 侧登记的「升级版」实现：
        // 补丁表里的既有条目不能改 YAML，但可以在 Rust 侧换更强的实现
        // （目前只有 window_title：放不下就搬到空洞）。
        if let Some(handler) = patches::upgrade_handler(&r.spec.id) {
            plan.handler = Some(handler.to_string());
            plan.reason = Some(format!("自定义补丁 {handler}（Rust 侧升级实现）：在写入阶段执行"));
            plans.push(plan);
            continue;
        }

        // 自定义补丁：放到写入阶段跑
        if r.mode == Mode::Custom {
            plan.handler = r.spec.handler.clone();
            plan.reason = Some("自定义补丁（在写入阶段执行）".into());
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
                    note: None,
                });
                plan.status = Status::Already;
                plan.already_at = Some(off);
                plan.reason = Some("未找到原始特征串，但替换后的字节已存在（已打过补丁）".into());
            } else {
                plan.status = Status::Fail;
                plan.reason = Some("未找到 find 模式".into());
                plan.not_applicable = true;
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
                    note: None,
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
                    note: None,
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

    // ---------------- 第五阶段：自定义补丁（Rust 处理器） ----------------
    let mut backup: Option<(Vec<u8>, crate::pe::Pe)> = None;
    for i in 0..plans.len() {
        let Some(handler) = plans[i].handler.clone() else {
            continue;
        };
        let spec = &table.patches[i];
        // dry-run：先备份引擎，写完再还原
        if opts.dry_run && backup.is_none() {
            backup = Some((engine.data.clone(), engine.pe.clone()));
        }
        let (outcome, writes, marker, summary) = {
            let mut ctx = Ctx::new(&mut engine, &plans[i].vals, plans[i].enc, &spec.id);
            let outcome = patches::dispatch(&handler, &mut ctx, spec);
            let writes = std::mem::take(&mut ctx.writes);
            (outcome, writes, ctx.marker, ctx.summary)
        };
        match outcome {
            Ok(Outcome::Applied) => {
                plans[i].hits = writes.len();
                plans[i].targets = writes
                    .into_iter()
                    .map(|w| Target {
                        offset: w.offset,
                        va: engine.va_of_offset(w.offset),
                        old: w.old,
                        new: w.new,
                        note: w.note,
                    })
                    .collect();
                plans[i].status = Status::Applied;
                plans[i].reason = Some(match summary {
                    Some(s) => format!("自定义补丁 {handler}：写入 {} 处；{s}", plans[i].targets.len()),
                    None => format!("自定义补丁 {handler}：写入 {} 处", plans[i].targets.len()),
                });
            }
            Ok(Outcome::Already) => {
                plans[i].hits = writes.len();
                plans[i].targets = writes
                    .into_iter()
                    .map(|w| Target {
                        offset: w.offset,
                        va: engine.va_of_offset(w.offset),
                        old: w.old,
                        new: w.new,
                        note: w.note,
                    })
                    .collect();
                plans[i].status = Status::Already;
                plans[i].reason = Some(match summary {
                    Some(s) => format!("已经是目标状态（幂等）；{s}"),
                    None => "已经是目标状态（幂等）".into(),
                });
                plans[i].already_at = marker;
            }
            Ok(Outcome::Fail(why)) => {
                plans[i].status = Status::Fail;
                plans[i].reason = Some(why);
                // 「找不到特征 —— 本版本不适用」算「不适用」，不是「补丁写坏了」
                if plans[i]
                    .reason
                    .as_deref()
                    .unwrap_or("")
                    .contains("找不到特征")
                {
                    plans[i].not_applicable = true;
                }
            }
            Ok(Outcome::Skipped(why)) => {
                plans[i].status = Status::Skipped;
                plans[i].reason = Some(why);
            }
            Err(e) => {
                plans[i].status = Status::Fail;
                plans[i].reason = Some(format!("{e:#}"));
            }
        }
    }
    if let Some((data, pe)) = backup {
        engine.data = data;
        engine.pe = pe;
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
                    note: t.note.clone(),
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
            if plan.targets.is_empty() {
                // 处理器没给落点 → 只标一个位置（和 M2 的老行为一样）
                let off = plan.already_at.unwrap_or(0);
                let old = hex::format_bytes(&engine.data[off..(off + 1).min(engine.data.len())]);
                locations.push(Location {
                    offset: off,
                    offset_hex: format!("0x{off:08X}"),
                    va: engine.va_of_offset(off),
                    va_hex: engine.va_of_offset(off).map(|v| format!("0x{v:08X}")),
                    section: engine.pe.section_of_offset(off).map(|s| s.name.clone()),
                    old: old.clone(),
                    new: String::new(),
                    status: Status::Already.name().to_string(),
                    note: None,
                });
                first = Some((off, engine.va_of_offset(off), old.clone(), String::new()));
            } else {
                // 处理器报出了落点（老 → 新，值相同 = 已经是目标状态）
                for t in &plan.targets {
                    let old = hex::format_bytes(&t.old);
                    let new = hex::format_bytes(&t.new);
                    locations.push(Location {
                        offset: t.offset,
                        offset_hex: format!("0x{:08X}", t.offset),
                        va: t.va,
                        va_hex: t.va.map(|v| format!("0x{v:08X}")),
                        section: engine.pe.section_of_offset(t.offset).map(|s| s.name.clone()),
                        old: old.clone(),
                        new: new.clone(),
                        status: Status::Already.name().to_string(),
                        note: t.note.clone(),
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
            }
            n_already += 1;
        } else if plan.status == Status::Skipped {
            n_skipped += 1;
        } else {
            n_failed += 1;
        }

        entries.push(Entry {
            id: spec.id.clone(),
            desc: spec.desc.clone(),
            desc_en: spec.desc_en.clone(),
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
            encoding: encodings.get(i).cloned().unwrap_or_default(),
            encoding_source: enc_sources.get(i).cloned().unwrap_or_default(),
            params: patch_params.get(i).cloned().unwrap_or_default(),
            not_applicable: plan.not_applicable,
        });
    }

    let n_not_applicable = plans.iter().filter(|p| p.not_applicable).count();
    let mut notes: Vec<String> = Vec::new();
    if n_not_applicable > 0 {
        notes.push(format!(
            "{n_not_applicable} 条不适用（客户端版本不符？）"
        ));
    }
    // 有的补丁用 ask 选了别的编码 → 提醒一句（表级编码和实际写入的不一样）
    if encodings.iter().any(|e| e != &table_enc.name().to_string()) {
        notes.push(format!(
            "部分补丁用了和表级不同的写入编码（表级 {}，逐条看 entries[].encoding）",
            table_enc.name()
        ));
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
        encoding: table_enc.name().to_string(),
        params: param_records,
        not_applicable: n_not_applicable,
        notes,
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

/// 解析所有补丁声明的参数（同一个 key 只问一次）。
fn resolve_params(table: &PatchTable, resolver: &mut ParamResolver) -> Result<ParamValues> {
    let mut values = ParamValues::new();
    for spec in &table.patches {
        if spec.disabled {
            continue;
        }
        let Some(ask) = &spec.ask else { continue };
        let rec = resolver.resolve(&spec.id, ask)?;
        values.insert(rec.key.clone(), rec.value.clone());
    }
    Ok(values)
}

/// `--set` 给了没人用的参数名 → 提醒一句（多半是打错了）。
pub fn warn_unused_sets(table: &PatchTable, resolver: &ParamResolver) {
    // 参数名（ask.key）和补丁 id 都能用来传参
    let declared: std::collections::BTreeSet<&str> = table
        .patches
        .iter()
        .filter_map(|s| s.ask.as_ref().map(|a| a.key.as_str()))
        .collect();
    let ids: std::collections::BTreeSet<&str> =
        table.patches.iter().map(|s| s.id.as_str()).collect();
    for k in resolver.cli_sets().keys() {
        if !declared.contains(k.as_str()) && !ids.contains(k.as_str()) {
            eprintln!("警告：--set {k}=... 没有任何补丁用这个参数名（也没有这样的补丁 id）");
        }
    }
}

/// 表级写入编码：命令行 > 表头 meta > 默认 gbk。
fn table_encoding(table: &PatchTable, cli: Option<Encoding>) -> Result<Encoding> {
    if let Some(e) = cli {
        return Ok(e);
    }
    match table.meta.as_ref().and_then(|m| m.encoding()) {
        Some(name) => Encoding::from_name(name).map_err(|_| {
            anyhow::anyhow!(
                "补丁表 meta.encoding 的值「{name}」不是支持的编码（期望 gbk / cp949 / utf8 / sjis / cp874）"
            )
        }),
        None => Ok(Encoding::Gbk),
    }
}

/// 挑一条补丁实际用的写入编码。
///
/// 优先级（从高到低）：
///
/// 1. 命令行 `--encoding`
/// 2. 本条补丁 ask 出来的值（`ask.key == "encoding"`，一般是 `kind: enum`）
/// 3. 本条补丁的 `encoding:` 字段
/// 4. 表头 `meta.encoding`
/// 5. 默认 gbk
fn pick_encoding(
    spec: &PatchSpec,
    meta: Option<&TableMeta>,
    cli: Option<Encoding>,
    values: &ParamValues,
) -> Result<(Encoding, Source)> {
    if let Some(e) = cli {
        return Ok((e, Source::Cli));
    }
    if let Some(ask) = &spec.ask {
        if ask.key == "encoding" {
            if let Some(v) = values.get("encoding") {
                let e = Encoding::from_name(v).map_err(|_| {
                    anyhow::anyhow!(
                        "参数 encoding 的值「{v}」不是支持的编码（期望 gbk / cp949 / utf8 / sjis / cp874）"
                    )
                })?;
                return Ok((e, Source::Ask));
            }
        }
    }
    if let Some(name) = &spec.encoding {
        let e = Encoding::from_name(name).map_err(|_| {
            anyhow::anyhow!(
                "补丁 {} 的 encoding 的值「{name}」不是支持的编码（期望 gbk / cp949 / utf8 / sjis / cp874）",
                spec.id
            )
        })?;
        return Ok((e, Source::Spec));
    }
    if let Some(name) = meta.and_then(|m| m.encoding()) {
        let e = Encoding::from_name(name).map_err(|_| {
            anyhow::anyhow!(
                "补丁表 meta.encoding 的值「{name}」不是支持的编码（期望 gbk / cp949 / utf8 / sjis / cp874）"
            )
        })?;
        return Ok((e, Source::Meta));
    }
    Ok((Encoding::Gbk, Source::Default))
}

/// 模板里用到了某个参数吗？
fn template_uses(template: &str, key: &str) -> bool {
    params::split_template(template)
        .iter()
        .any(|p| matches!(p, params::Part::Placeholder(k) if k == key))
}

/// 解析一条补丁（找模式、替换内容、搜索范围、编码、参数）。
fn resolve_patch(
    engine: &Engine,
    spec: &PatchSpec,
    meta: Option<&TableMeta>,
    cli_encoding: Option<Encoding>,
    values: &ParamValues,
    resolver: &ParamResolver,
) -> Result<Resolved> {
    let mode = spec.mode()?;
    let (encoding, encoding_source) = pick_encoding(spec, meta, cli_encoding, values)?;
    let who = format!("补丁 {}", spec.id);

    // ---- find：hex，或者带 {{参数}} 的模板
    let find = if mode == Mode::Custom {
        Vec::new()
    } else if params::has_placeholder(&spec.find) {
        let bytes = params::render_hex(&spec.find, values, encoding, &who)?;
        bytes
            .into_iter()
            .map(|b| PatByte {
                value: b,
                mask: 0xFF,
            })
            .collect()
    } else {
        hex::parse_pattern(&spec.find)
            .with_context(|| format!("find 模式解析失败：{}", spec.find))?
    };
    let range = engine.range_of(spec.section.as_deref())?;

    let mut repl: Vec<u8> = Vec::new();
    let mut repl_pat: Vec<Option<u8>> = Vec::new();
    let mut terminate = spec.terminate.unwrap_or(false);
    match mode {
        Mode::Replace => {
            // replace：hex（允许 ?? 通配），也可以是带 {{参数}} 的 hex 模板
            let replace_pat: Option<Vec<Option<u8>>> = match &spec.replace {
                Some(h) if !params::has_placeholder(h) => Some(
                    hex::parse_replace(h)
                        .with_context(|| format!("replace 字节串解析失败：{h}"))?,
                ),
                _ => None,
            };
            let replace_bytes: Option<Vec<u8>> = match (&spec.replace, &replace_pat) {
                (Some(h), _) if params::has_placeholder(h) => {
                    Some(params::render_hex(h, values, encoding, &who)?)
                }
                (_, Some(pat)) => Some(pat.iter().map(|o| o.unwrap_or(0)).collect()),
                _ => None,
            };
            // text：UTF-8 明文（也可以带 {{参数}}），按落地编码转码
            let text_bytes: Option<Vec<u8>> = match &spec.text {
                Some(t) => Some(text_to_bytes(t, values, encoding, &who)?),
                None => None,
            };

            match (text_bytes, replace_bytes.clone()) {
                (Some(tb), Some(rb)) => {
                    // 两个都给：text 生效，replace 作为校验对照
                    check_text_vs_replace(spec, &tb, &rb, &replace_pat)?;
                    repl = tb;
                }
                (Some(tb), None) => {
                    repl = tb;
                    // 文本补丁默认补字符串结束符（和 WARP 的 SetText 一致）
                    if spec.terminate.is_none() {
                        terminate = true;
                    }
                }
                (None, Some(_)) => {
                    // 纯 hex 替换（可能带 ?? 通配）
                    if let Some(pat) = replace_pat {
                        repl_pat = pat;
                    } else {
                        repl = replace_bytes.unwrap_or_default();
                    }
                }
                (None, None) => {
                    bail!("补丁 {} 既没有 replace 也没有 text", spec.id);
                }
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
        Mode::Custom => {
            if spec.handler.is_none() {
                bail!("补丁 {} 是 custom 模式，但没有 handler", spec.id);
            }
            terminate = false;
        }
    }

    // ---- 这条补丁用到的参数（写进报告）
    let mut used: Vec<ParamRecord> = Vec::new();
    let mut param_bytes: Option<(String, usize)> = None;
    let mut param_bytes_all: Vec<(String, usize)> = Vec::new();
    for (k, v) in values {
        let used_here = template_uses(&spec.find, k)
            || spec.replace.as_deref().map(|t| template_uses(t, k)).unwrap_or(false)
            || spec.text.as_deref().map(|t| template_uses(t, k)).unwrap_or(false);
        if !used_here {
            continue;
        }
        let n = encode::encode_for(v, encoding, k)?.len();
        param_bytes_all.push((k.clone(), n));
        if param_bytes.as_ref().map(|(_, m)| n > *m).unwrap_or(true) {
            param_bytes = Some((k.clone(), n));
        }
        if let Some(rec) = resolver.get(k) {
            let mut rec = rec.clone();
            rec.bytes = Some(n);
            used.push(rec);
        }
    }
    if let Some(ask) = &spec.ask {
        if let Some(rec) = resolver.get(&ask.key) {
            if !used.iter().any(|r| r.key == rec.key) {
                used.push(rec.clone());
            }
        }
    }

    Ok(Resolved {
        spec: spec.clone(),
        mode,
        find,
        repl,
        repl_pat,
        terminate,
        zero_fill: spec.zero_fill.unwrap_or(false),
        range,
        encoding,
        encoding_source,
        params: used,
        param_bytes,
        param_bytes_all,
        param_values: values.clone(),
    })
}

/// `text:`（UTF-8 明文）→ 落地字节。
///
/// 先逐个校验参数（报错要指出是哪个 key 的哪个字符），再整体转码。
fn text_to_bytes(
    template: &str,
    values: &ParamValues,
    encoding: Encoding,
    who: &str,
) -> Result<Vec<u8>> {
    for (k, v) in values {
        if template_uses(template, k) {
            encode::encode_for(v, encoding, k)?;
        }
    }
    let text = params::render_text(template, values, who)?;
    encode::encode_for(&text, encoding, who)
}

/// `text:` 和 `replace:` 都给时：text 生效，replace 作为校验对照。
fn check_text_vs_replace(
    spec: &PatchSpec,
    text_bytes: &[u8],
    replace_bytes: &[u8],
    replace_pat: &Option<Vec<Option<u8>>>,
) -> Result<()> {
    let mismatch = match replace_pat {
        // 有 ?? 通配：只比对固定位
        Some(pat) => pat.iter().enumerate().any(|(i, o)| match o {
            Some(v) => text_bytes.get(i) != Some(v),
            None => false,
        }),
        None => text_bytes != replace_bytes,
    };
    ensure!(
        !mismatch,
        "补丁 {} 同时给了 text 和 replace，但两者对不上（text 转码后 {}，replace {}）；以 text 为准时请把 replace 改成一样的",
        spec.id,
        hex::format_bytes(text_bytes),
        hex::format_bytes(replace_bytes)
    );
    Ok(())
}

/// 从 `from` 开始连续有多少个 0x00（用来算「借用写入预算」）。
fn nul_padding(engine: &Engine, from: usize) -> usize {
    let mut n = 0usize;
    let mut i = from;
    while i < engine.data.len() && engine.data[i] == 0x00 {
        n += 1;
        i += 1;
    }
    n
}

/// 算一个落点要写什么字节，并做长度 / 边界校验。
fn compute_target(engine: &Engine, r: &Resolved, off: usize) -> Result<Target> {
    let va = engine.va_of_offset(off);
    let new = match r.mode {
        Mode::Replace => {
            let mut buf = if r.repl_pat.is_empty() {
                r.repl.clone()
            } else {
                // `??` 的位置保留原字节
                let end = (off + r.repl_pat.len()).min(engine.data.len());
                hex::resolve_replace(&r.repl_pat, &engine.data[off..end])?
            };
            if r.terminate {
                buf.push(0x00);
            }
            // 写入预算：find 的长度 + 后面连续的 0x00 填充。
            // 例外：允许借用紧跟在匹配之后的连续 0x00 填充 —— 只覆盖 NUL，
            // 不碰任何非零数据，也不会跨段。
            let pad = nul_padding(engine, off + r.find.len());
            let limit = if r.zero_fill {
                r.find.len()
            } else {
                r.find.len() + pad
            };
            if buf.len() > limit {
                // 参数太长：明确报错，不静默截断
                if let Some((key, n)) = &r.param_bytes {
                    let max = limit.saturating_sub(if r.terminate { 1 } else { 0 });
                    bail!("参数 {key} 的文本太长（{n} 字节，最多 {max} 字节）");
                }
                if r.zero_fill {
                    bail!(
                        "替换字节数 {} 超过 find 的 {} 字节（zero_fill 要求整段覆写）",
                        buf.len(),
                        r.find.len()
                    );
                }
                bail!(
                    "替换字节数 {} 超过 find 的 {} 字节（尾部可用 NUL 填充 {} 字节）",
                    buf.len(),
                    r.find.len(),
                    pad
                );
            }
            if r.zero_fill {
                // 整段覆写：剩下的字节清零，不留残渣
                buf.resize(r.find.len(), 0);
            }
            if buf.len() > r.find.len() {
                // 借用填充时，落地范围必须仍在同一段内
                engine.check_write(off, buf.len())?;
            }
            buf
        }
        Mode::Nop => r.repl.clone(),
        Mode::Custom => unreachable!("custom 模式不走这里"),
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
        note: None,
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
    if !r.repl_pat.is_empty() {
        // 带通配的替换：把固定字节当特征去搜（通配位还是通配）
        let pat: Vec<PatByte> = r
            .repl_pat
            .iter()
            .map(|o| match o {
                Some(v) => PatByte {
                    value: *v,
                    mask: 0xFF,
                },
                None => PatByte {
                    value: 0,
                    mask: 0x00,
                },
            })
            .collect();
        let hits = engine.find_pattern(&pat, r.range);
        let off = *hits.first()?;
        let end = (off + r.repl_pat.len()).min(engine.data.len());
        return Some((off, engine.data[off..end].to_vec()));
    }
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
    /// 实际使用的写入编码。
    pub encoding: String,
    /// 本次用到的所有参数（值 + 来源）。
    pub params: Vec<ParamRecord>,
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
    /// 这条补丁实际用的写入编码。
    pub encoding: String,
    /// 编码的来源：cli / ask / spec / meta / default。
    pub encoding_source: String,
    /// 这条补丁用到的参数。
    pub params: Vec<ParamRecord>,
}

/// `verify` 的参数。
#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// 参数来源（`--set` / `--interactive`）。
    pub params: ParamResolver,
    /// 命令行指定的写入编码（必须和 apply 时一致）。
    pub encoding: Option<Encoding>,
}

/// 校验：每条补丁的替换内容是否已经在文件里。
///
/// 给了 `original` 时用原始文件定位落点（最准）；没给就退化成
/// 「替换字节是否出现在搜索范围里」。
pub fn verify(
    data: Vec<u8>,
    table: &PatchTable,
    original: Option<&[u8]>,
    opts: &mut VerifyOptions,
) -> Result<VerifyResult> {
    let engine = Engine::new(data)?;
    let orig_engine = match original {
        Some(o) => Some(Engine::new(o.to_vec())?),
        None => None,
    };
    let table_enc = table_encoding(table, opts.encoding)?;
    let param_values = resolve_params(table, &mut opts.params)?;
    warn_unused_sets(table, &opts.params);
    let param_records = opts.params.records();

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
            encoding: table_enc.name().to_string(),
            encoding_source: Source::Default.name().to_string(),
            params: Vec::new(),
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

        // 解析（和 apply 用同一套逻辑：同样的编码、同样的参数 → 同样的期望字节）
        let r = match resolve_patch(
            &engine,
            spec,
            table.meta.as_ref(),
            opts.encoding,
            &param_values,
            &opts.params,
        ) {
            Ok(r) => r,
            Err(e) => {
                entry.reason = Some(format!("{e:#}"));
                failed += 1;
                entries.push(entry);
                continue;
            }
        };
        entry.encoding = r.encoding.name().to_string();
        entry.encoding_source = r.encoding_source.name().to_string();
        entry.params = r.params.clone();
        let mode = r.mode;

        // 自定义补丁 / Rust 侧登记的升级实现：
        // 用处理器自己判断「是不是已经打上了」
        // （表驱动的 replace 只会原地比对字节，看不出「搬到空洞」那种情况）
        let upgrade = patches::upgrade_handler(&spec.id);
        let handler = match upgrade {
            Some(h) => Some(h.to_string()),
            None => {
                if mode == Mode::Custom {
                    match spec.handler.clone() {
                        Some(h) => Some(h),
                        None => {
                            entry.reason = Some("custom 模式但没有 handler".into());
                            failed += 1;
                            entries.push(entry);
                            continue;
                        }
                    }
                } else {
                    None
                }
            }
        };
        if let Some(handler) = handler {
            let mut probe = Engine::new(engine.data.clone())?;
            let (outcome, writes, summary) = {
                let mut ctx = Ctx::new(&mut probe, &r.param_values, r.encoding, &spec.id);
                ctx.check_only = true;
                let o = patches::dispatch(&handler, &mut ctx, spec);
                (o, ctx.writes, ctx.summary)
            };
            // 落点信息（报告里能看到「改的是哪几个字节」）
            if let Some(w) = writes.first() {
                entry.offset = Some(w.offset);
                entry.va = engine.va_of_offset(w.offset);
                entry.expected = Some(hex::format_bytes(&w.new));
                entry.actual = Some(hex::format_bytes(
                    &engine.data[w.offset..(w.offset + w.new.len()).min(engine.data.len())],
                ));
            }
            match outcome {
                Ok(Outcome::Already) => {
                    entry.status = "ok".into();
                    entry.reason = Some(match summary {
                        Some(s) => format!("补丁已生效；{s}"),
                        None => "补丁已生效".into(),
                    });
                    passed += 1;
                }
                Ok(Outcome::Applied) => {
                    entry.status = "fail".into();
                    entry.reason = Some("补丁还没打上（处理器认为可以写入）".into());
                    failed += 1;
                }
                Ok(Outcome::Fail(why)) => {
                    entry.status = "fail".into();
                    entry.reason = Some(why);
                    failed += 1;
                }
                Ok(Outcome::Skipped(why)) => {
                    entry.status = "skipped".into();
                    entry.reason = Some(why);
                    skipped += 1;
                }
                Err(e) => {
                    entry.status = "fail".into();
                    entry.reason = Some(format!("{e:#}"));
                    failed += 1;
                }
            }
            entries.push(entry);
            continue;
        }

        // 替换内容（和 apply 用同一份解析结果）
        let repl_pat: Vec<Option<u8>> = r.repl_pat.clone();
        let repl: Vec<u8> = r.repl.clone();
        let terminate = r.terminate;
        let find_pat = r.find.clone();
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
            let hits = orig.find_pattern(&find_pat, orig_range);
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
                // 有通配的替换：期望值 = 固定字节 + 原文件里那些位置的字节
                let expected = if !repl_pat.is_empty() {
                    let oend = (off + repl_pat.len()).min(orig.data.len());
                    hex::resolve_replace(&repl_pat, &orig.data[off..oend]).unwrap_or_default()
                } else {
                    expected.clone()
                };
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
        let found = if !repl_pat.is_empty() {
            // 带通配的替换：把固定字节当特征去搜
            let pat: Vec<PatByte> = repl_pat
                .iter()
                .map(|o| match o {
                    Some(v) => PatByte {
                        value: *v,
                        mask: 0xFF,
                    },
                    None => PatByte {
                        value: 0,
                        mask: 0x00,
                    },
                })
                .collect();
            engine.find_pattern(&pat, range).into_iter().next()
        } else if expected.is_empty() {
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
        encoding: table_enc.name().to_string(),
        params: param_records,
        entries,
    })
}
