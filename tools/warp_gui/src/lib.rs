//! warp_gui —— warp_rs 的图形前端（Tauri v2）：**纯逻辑层**。
//!
//! 这一层**不依赖 tauri**，因此可以：
//!
//! * 在 Linux / macOS 上直接单测（`cargo test`），不需要 WebView；
//! * 被任意桌面前端直接复用（同一套「扫描 → 预览 → 备份 → 打补丁 → 校验」流程）。
//!
//! | 函数 | 作用 |
//! |---|---|
//! | [`scan_tables`] | 扫描 `patches/*.yml`，列出补丁表（下拉框用） |
//! | [`load_table`] | 解析一张表，逐条算出「在当前 exe 上能不能定位」 |
//! | [`plan`] | 预览（dry-run）：只报告，不写文件 |
//! | [`apply_patch`] | 备份 → 打补丁 → 写 `<原名>_patched.exe` → 校验 |
//! | [`verify_patch`] | 校验一个已经打过补丁的 exe |
//!
//! 字节处理全部交给 `warp_rs` 库（不重写引擎，也不起子进程）。

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use warp_rs::apply::{self, ApplyOptions, VerifyOptions};
use warp_rs::engine::Engine;
use warp_rs::hex;
use warp_rs::params::ParamResolver;
use warp_rs::table::{PatchSpec, PatchTable};

// ---------------------------------------------------------------------------
// 数据结构（给前端看的 DTO）
// ---------------------------------------------------------------------------

/// 目标 exe 的基本信息（界面上「客户端 exe」那一行显示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExeInfo {
    /// 绝对路径。
    pub path: String,
    /// 文件名。
    pub name: String,
    /// 字节数。
    pub size: u64,
    /// md5（小写十六进制）。
    pub md5: String,
    /// 是不是 PE 文件（头两个字节是不是 `MZ`）。
    pub is_pe: bool,
}

/// 扫描补丁表的结果（目录 + 表列表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// 补丁表目录（界面上显示，方便用户知道扫的是哪儿）。
    pub dir: String,
    /// 表列表。
    pub tables: Vec<TableInfo>,
}

/// 一张补丁表的概览（下拉框里的一项）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    /// 文件名（相对补丁目录），下拉框显示它。
    pub file: String,
    /// 补丁条数（表坏了是 0）。
    pub count: usize,
    /// 表头声明的写入编码（没有就是 `gbk`）。
    pub encoding: String,
    /// 解析错误（表坏掉也列出来，方便发现）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 一条补丁（界面表格的一行）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRow {
    /// 补丁 id。
    pub id: String,
    /// 中文说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// 英文原文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc_en: Option<String>,
    /// 补丁模式：replace / nop / jmp / custom / offset。
    pub mode: String,
    /// 文件偏移（`0x…`）。`mode: offset` 是表里写死的；其它模式是特征在 exe 里的命中位置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
    /// 在当前 exe 上能不能定位到。
    pub available: bool,
    /// 可用性说明（找不到 / 命中几处 / 由引擎判定……）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_note: Option<String>,
    /// 补丁表里就写死了「不启用」。
    pub disabled: bool,
}

/// 一张补丁表的解析结果（界面表格 + 汇总）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableView {
    /// 文件名。
    pub file: String,
    /// 表头声明的写入编码。
    pub encoding: String,
    /// 共 N 条。
    pub total: usize,
    /// 逐条明细。
    pub entries: Vec<EntryRow>,
}

/// 一次预览的结果（dry-run）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanResult {
    /// 共 N 条（表里的）。
    pub total: usize,
    /// 已选 M 条。
    pub selected: usize,
    /// 会真正写进去的条数。
    pub will_apply: usize,
    /// 会跳过的条数。
    pub will_skip: usize,
    /// 会失败的条数。
    pub failed: usize,
    /// 在当前 exe 上找不到的条数（会跳过）。
    pub not_found: usize,
    /// 逐条明细。
    pub rows: Vec<PlanRow>,
    /// 引擎给的提示。
    pub notes: Vec<String>,
}

/// 预览 / 应用结果里的一条补丁。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRow {
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
    /// 落点偏移。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
    /// 这条补丁在当前 exe 上找不到特征。
    pub not_applicable: bool,
}

/// 一次应用（真打补丁）的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    /// 备份文件路径。
    pub backup: String,
    /// 输出文件路径。
    pub out: String,
    /// 输出文件字节数。
    pub out_size: u64,
    /// 输出文件 md5。
    pub out_md5: String,
    /// 打上多少条。
    pub applied: usize,
    /// 已经打过多少条。
    pub already: usize,
    /// 跳过多少条。
    pub skipped: usize,
    /// 失败多少条。
    pub failed: usize,
    /// 在当前 exe 上找不到的条数。
    pub not_found: usize,
    /// 校验通过多少条。
    pub verify_passed: usize,
    /// 校验失败多少条。
    pub verify_failed: usize,
    /// 校验跳过多少条。
    pub verify_skipped: usize,
    /// 校验整体通过。
    pub verify_ok: bool,
    /// 逐条明细（应用结果）。
    pub rows: Vec<PlanRow>,
    /// 引擎给的提示。
    pub notes: Vec<String>,
}

/// 一次校验的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    /// 被校验的文件。
    pub target: String,
    /// 整体通过。
    pub ok: bool,
    /// 共 N 条。
    pub total: usize,
    /// 通过多少条。
    pub passed: usize,
    /// 失败多少条。
    pub failed: usize,
    /// 跳过多少条。
    pub skipped: usize,
    /// 逐条明细。
    pub rows: Vec<PlanRow>,
}

// ---------------------------------------------------------------------------
// md5
// ---------------------------------------------------------------------------

/// 算 md5（小写十六进制）。
pub fn md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data);
    let sum = h.finalize();
    let mut s = String::with_capacity(32);
    for b in sum.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 算文件的 md5。
pub fn md5_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("读文件失败：{}", path.display()))?;
    Ok(md5_hex(&data))
}

/// 读目标 exe 的基本信息（界面上的「客户端 exe」那一行）。
pub fn exe_info(path: &Path) -> Result<ExeInfo> {
    let data = std::fs::read(path).with_context(|| format!("读客户端失败：{}", path.display()))?;
    Ok(ExeInfo {
        path: path.display().to_string(),
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        size: data.len() as u64,
        md5: md5_hex(&data),
        is_pe: data.starts_with(b"MZ"),
    })
}

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/// 补丁表目录的候选位置（按优先级）。
///
/// 1. 环境变量 `WARP_GUI_PATCHES_DIR`（部署时最省事）；
/// 2. 当前工作目录 / 其上级的 `patches/`；
/// 3. 可执行文件所在目录 / 其上级的 `patches/`；
/// 4. 可执行文件旁边的 `patches/`（把补丁表和 exe 放一起发出去）。
///
/// 找到第一个「存在且里有 yml」的目录就返回。
pub fn find_patches_dir() -> Option<PathBuf> {
    // 1) 环境变量优先
    if let Ok(dir) = std::env::var("WARP_GUI_PATCHES_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }

    // 2) 从当前工作目录往上找
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(p) = search_upwards(&cwd) {
            return Some(p);
        }
    }

    // 3) 从可执行文件所在目录往上找
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(p) = search_upwards(dir) {
                return Some(p);
            }
        }
    }

    None
}

/// 从 `start` 一路往上级目录找 `patches/`。
fn search_upwards(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start.to_path_buf());
    for _ in 0..8 {
        let dir = cur?;
        let cand = dir.join("patches");
        if cand.is_dir() {
            return Some(cand);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// 输出文件路径：同目录 `<原名>_patched.exe`。
pub fn patched_path(exe: &Path) -> PathBuf {
    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "client".to_string());
    let ext = exe
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "exe".to_string());
    exe.with_file_name(format!("{stem}_patched.{ext}"))
}

/// 备份文件名。
///
/// * 备份还不存在 → `<原名>.bak`
/// * 备份已存在 → `<原名>.bak.<时间戳>`（时间戳形如 `20250624-153012`）
///
/// 抽成纯函数是为了能单测（不碰文件系统）。
pub fn backup_file_name(stem: &str, bak_exists: bool, stamp: &str) -> String {
    if bak_exists {
        format!("{stem}.bak.{stamp}")
    } else {
        format!("{stem}.bak")
    }
}

/// 当前时间戳（UTC，格式 `YYYYMMDD-HHMMSS`；只用来区分备份文件名）。
pub fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 用 UTC 算，避免引入 chrono/time 依赖（时间戳只用来区分文件名）。
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// 天数 → （年，月，日）。Howard Hinnant 的 civil_from_days 算法。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 备份文件路径（`bak_exists` 由调用方判断）。
pub fn backup_path(exe: &Path, bak_exists: bool, stamp: &str) -> PathBuf {
    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "client".to_string());
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    dir.join(backup_file_name(&stem, bak_exists, stamp))
}

// ---------------------------------------------------------------------------
// 扫描补丁表
// ---------------------------------------------------------------------------

/// 扫描目录里的 `*.yml` / `*.yaml`（不递归）。
pub fn scan_tables(dir: &Path) -> Result<Vec<TableInfo>> {
    if !dir.is_dir() {
        bail!("补丁表目录不存在：{}", dir.display());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("读目录失败：{}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    Ok(files.iter().map(|f| table_info_of(f)).collect())
}

/// 扫描补丁表目录（目录 + 表列表）。
pub fn scan(dir: &Path) -> Result<ScanResult> {
    Ok(ScanResult {
        dir: dir.display().to_string(),
        tables: scan_tables(dir)?,
    })
}

/// 读一张表的信息（坏表也返回，只是带 error）。
fn table_info_of(path: &Path) -> TableInfo {
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match PatchTable::load(path) {
        Ok(t) => TableInfo {
            file,
            count: t.patches.len(),
            encoding: t
                .meta
                .as_ref()
                .and_then(|m| m.encoding.clone())
                .unwrap_or_else(|| "gbk".to_string()),
            error: None,
        },
        Err(e) => TableInfo {
            file,
            count: 0,
            encoding: String::new(),
            error: Some(format!("{e:#}")),
        },
    }
}

// ---------------------------------------------------------------------------
// 解析一张表（逐条算可用性）
// ---------------------------------------------------------------------------

/// 解析一张补丁表，逐条算出「在当前 exe 上能不能定位」。
///
/// * `exe` 传 `None` → 只解析表，可用性一律标成「未判定」。
/// * `exe` 传了但不是合法 PE → 报错（界面上一行中文说明）。
pub fn load_table(path: &Path, exe: Option<&Path>) -> Result<TableView> {
    let table = PatchTable::load(path)?;
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let encoding = table
        .meta
        .as_ref()
        .and_then(|m| m.encoding.clone())
        .unwrap_or_else(|| "gbk".to_string());

    // 有客户端 exe 就把它的字节读进来（探测特征用）
    let engine = match exe {
        Some(p) => {
            let data = std::fs::read(p)
                .with_context(|| format!("读客户端失败：{}", p.display()))?;
            Some(Engine::new(data).with_context(|| {
                format!("{} 看起来不是合法的 PE 客户端文件", p.display())
            })?)
        }
        None => None,
    };

    let entries = table
        .patches
        .iter()
        .map(|spec| entry_row(spec, engine.as_ref()))
        .collect();

    Ok(TableView {
        file,
        encoding,
        total: table.patches.len(),
        entries,
    })
}

/// 一条补丁 → 界面的一行（含可用性探测）。
fn entry_row(spec: &PatchSpec, engine: Option<&Engine>) -> EntryRow {
    let mode = spec.mode().map(|m| m.name().to_string()).unwrap_or_else(|_| "replace".into());
    let (available, note, offset) = probe(spec, engine);

    EntryRow {
        id: spec.id.clone(),
        desc: spec.desc.clone(),
        desc_en: spec.desc_en.clone(),
        mode,
        offset,
        available,
        available_note: note,
        disabled: spec.disabled,
    }
}

/// 探测一条补丁在当前 exe 上能不能定位。
///
/// 返回 `(能不能定位, 说明, 落点偏移)`。
fn probe(spec: &PatchSpec, engine: Option<&Engine>) -> (bool, Option<String>, Option<String>) {
    // 表里就写死了不启用
    if spec.disabled {
        return (
            false,
            Some(spec.note.clone().unwrap_or_else(|| "补丁表里已标记为不启用".into())),
            None,
        );
    }
    // `mode: offset` 的偏移是表里写死的
    let fixed = spec.offset.as_ref().map(|o| format!("0x{}", o.trim()));

    let Some(engine) = engine else {
        return (
            true,
            Some("未选择客户端 exe，可用性未判定".into()),
            fixed,
        );
    };

    // 没有特征串（custom / offset）：交给引擎在预览时判定
    let find = spec.find.trim();
    if find.is_empty() {
        return (
            true,
            Some("自定义 / 偏移补丁，由引擎在预览时判定".into()),
            fixed,
        );
    }

    let pat = match hex::parse_pattern(find) {
        Ok(p) => p,
        Err(e) => return (false, Some(format!("特征串解析失败：{e}")), fixed),
    };
    let range = match engine.range_of(spec.section.as_deref()) {
        Ok(r) => r,
        Err(e) => return (false, Some(format!("定位段失败：{e}")), fixed),
    };
    let hits = engine.find_pattern(&pat, range);
    if hits.is_empty() {
        return (
            false,
            Some("当前 exe 里找不到这条补丁的特征（会跳过）".into()),
            fixed,
        );
    }
    (
        true,
        Some(format!("命中 {} 处", hits.len())),
        Some(format!("0x{:08X}", hits[0])),
    )
}

// ---------------------------------------------------------------------------
// 预览 / 应用 / 校验
// ---------------------------------------------------------------------------

/// 按 id 过滤一张表（保持表里的顺序）。
fn filtered_table(path: &Path, ids: &[String]) -> Result<PatchTable> {
    let mut table = PatchTable::load(path)?;
    let want: BTreeSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    table.patches.retain(|p| want.contains(p.id.as_str()));
    Ok(table)
}

/// 把引擎的 `Entry` 转成界面的一行。
fn row_of(e: &warp_rs::apply::Entry) -> PlanRow {
    PlanRow {
        id: e.id.clone(),
        desc: e.desc.clone(),
        status: e.status.clone(),
        reason: e.reason.clone(),
        offset: e.offset_hex.clone(),
        not_applicable: e.not_applicable,
    }
}

/// 汇总一批预览结果（纯计算，方便单测）。
///
/// * `will_apply` —— 会写进去的（`applied`）+ 已经是目标状态的（`already`）
/// * `will_skip`  —— 跳过的（`skipped`）
/// * `failed`      —— 失败的（`fail`）
/// * `not_found`   —— 在当前 exe 上找不到特征的（会跳过）
pub fn summarize(rows: &[PlanRow], selected: usize, total: usize) -> PlanSummary {
    let mut s = PlanSummary {
        total,
        selected,
        will_apply: 0,
        will_skip: 0,
        failed: 0,
        not_found: 0,
    };
    for r in rows {
        match r.status.as_str() {
            "applied" | "already" => s.will_apply += 1,
            "skipped" => s.will_skip += 1,
            _ => s.failed += 1,
        }
        if r.not_applicable {
            s.not_found += 1;
        }
    }
    s
}

/// 预览（dry-run）：只算 plan，**不写任何文件**。
pub fn plan(exe: &Path, table_path: &Path, ids: &[String]) -> Result<PlanResult> {
    if ids.is_empty() {
        bail!("没有勾选任何补丁");
    }
    let total = PatchTable::load(table_path)?.patches.len();
    let table = filtered_table(table_path, ids)?;
    let data =
        std::fs::read(exe).with_context(|| format!("读客户端失败：{}", exe.display()))?;

    let mut opts = ApplyOptions {
        out: None,
        dry_run: true,
        allow_partial: true,
        params: ParamResolver::new(Vec::new(), false),
        encoding: None,
    };
    let (report, _) = apply::apply(data, &table, &mut opts)?;
    let rows: Vec<PlanRow> = report.entries.iter().map(row_of).collect();
    let sum = summarize(&rows, ids.len(), total);

    Ok(PlanResult {
        total: sum.total,
        selected: sum.selected,
        will_apply: sum.will_apply,
        will_skip: sum.will_skip,
        failed: sum.failed,
        not_found: sum.not_found,
        rows,
        notes: report.notes,
    })
}

/// 应用：备份原 exe → 打补丁 → 写 `<原名>_patched.exe` → 校验。
pub fn apply_patch(exe: &Path, table_path: &Path, ids: &[String]) -> Result<ApplyResult> {
    if ids.is_empty() {
        bail!("没有勾选任何补丁");
    }
    let original =
        std::fs::read(exe).with_context(|| format!("读客户端失败：{}", exe.display()))?;
    let table = filtered_table(table_path, ids)?;

    // 1) 先在内存里打补丁（引擎不碰磁盘；失败就什么都不落地）
    let mut opts = ApplyOptions {
        out: None,
        dry_run: false,
        allow_partial: false,
        params: ParamResolver::new(Vec::new(), false),
        encoding: None,
    };
    let (report, bytes) = apply::apply(original.clone(), &table, &mut opts)?;
    let Some(bytes) = bytes else {
        let failed: Vec<String> = report
            .entries
            .iter()
            .filter(|e| e.status == "fail")
            .map(|e| format!("{}（{}）", e.id, e.reason.clone().unwrap_or_default()))
            .collect();
        bail!(
            "有补丁失败，整份补丁没有落盘：{}",
            if failed.is_empty() {
                "未知原因".to_string()
            } else {
                failed.join("、")
            }
        );
    };

    // 2) 备份原 exe（已存在就加时间戳，绝不覆盖老备份）
    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "client".to_string());
    let bak = backup_path(
        exe,
        exe.with_file_name(format!("{stem}.bak")).exists(),
        &timestamp_now(),
    );
    std::fs::copy(exe, &bak).with_context(|| format!("备份失败：{}", bak.display()))?;

    // 3) 写输出文件
    let out = patched_path(exe);
    std::fs::write(&out, &bytes).with_context(|| format!("写输出失败：{}", out.display()))?;

    // 4) 校验（用原文件定位落点，最准）
    let mut vopts = VerifyOptions {
        params: ParamResolver::new(Vec::new(), false),
        encoding: None,
    };
    let verify = apply::verify(bytes.clone(), &table, Some(&original), &mut vopts)
        .with_context(|| "校验失败".to_string())?;

    let rows: Vec<PlanRow> = report.entries.iter().map(row_of).collect();
    Ok(ApplyResult {
        backup: bak.display().to_string(),
        out: out.display().to_string(),
        out_size: bytes.len() as u64,
        out_md5: md5_hex(&bytes),
        applied: report.applied,
        already: report.already,
        skipped: report.skipped,
        failed: report.failed,
        not_found: report.not_applicable,
        verify_passed: verify.passed,
        verify_failed: verify.failed,
        verify_skipped: verify.skipped,
        verify_ok: verify.ok,
        rows,
        notes: report.notes,
    })
}

/// 校验的默认目标：优先用 `<原名>_patched.exe`（存在的话），否则用原 exe。
pub fn verify_target(exe: &Path) -> PathBuf {
    let patched = patched_path(exe);
    if patched.exists() {
        patched
    } else {
        exe.to_path_buf()
    }
}

/// 校验一个已经打过补丁的 exe。
pub fn verify_patch(exe: &Path, table_path: &Path, ids: &[String]) -> Result<VerifyResult> {
    if ids.is_empty() {
        bail!("没有勾选任何补丁");
    }
    let data =
        std::fs::read(exe).with_context(|| format!("读客户端失败：{}", exe.display()))?;
    let table = filtered_table(table_path, ids)?;
    let mut vopts = VerifyOptions {
        params: ParamResolver::new(Vec::new(), false),
        encoding: None,
    };
    let res = apply::verify(data, &table, None, &mut vopts)?;
    let rows = res
        .entries
        .iter()
        .map(|e| PlanRow {
            id: e.id.clone(),
            desc: e.desc.clone(),
            status: e.status.clone(),
            reason: e.reason.clone(),
            offset: e.offset.map(|o| format!("0x{o:08X}")),
            not_applicable: false,
        })
        .collect();
    Ok(VerifyResult {
        target: exe.display().to_string(),
        ok: res.ok,
        total: res.total,
        passed: res.passed,
        failed: res.failed,
        skipped: res.skipped,
        rows,
    })
}

// ---------------------------------------------------------------------------
// 汇总的中间结构
// ---------------------------------------------------------------------------

/// 预览结果的汇总（[`summarize`] 的产物）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanSummary {
    /// 表里共 N 条。
    pub total: usize,
    /// 已选 M 条。
    pub selected: usize,
    /// 会写进去的。
    pub will_apply: usize,
    /// 会跳过的。
    pub will_skip: usize,
    /// 会失败的。
    pub failed: usize,
    /// 在当前 exe 上找不到的。
    pub not_found: usize,
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张临时补丁表目录。
    fn temp_tables() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warp_gui_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("base.yml"),
            "version: 1\nmeta: {encoding: gbk}\npatches:\n  - {id: a, desc: 甲}\n  - {id: b, desc: 乙}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("extra.yml"),
            "version: 1\npatches:\n  - {id: c, desc: 丙}\n",
        )
        .unwrap();
        // 坏表也要能被扫出来（带 error，不影响别的表）
        std::fs::write(dir.join("broken.yml"), "version: 999\npatches: []\n").unwrap();
        dir
    }

    #[test]
    fn 扫描补丁表() {
        let dir = temp_tables();
        let list = scan_tables(&dir).unwrap();
        // 按文件名排序：base / broken / extra
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].file, "base.yml");
        assert_eq!(list[0].count, 2);
        assert_eq!(list[0].encoding, "gbk");
        assert_eq!(list[1].file, "broken.yml");
        assert!(list[1].error.is_some(), "坏表应带错误说明");
        assert_eq!(list[2].file, "extra.yml");
        assert_eq!(list[2].count, 1);
        // 没有 yml 的目录不该报错，只是空
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(scan_tables(&empty).unwrap().is_empty());
    }

    #[test]
    fn plan汇总计算() {
        let rows = vec![
            PlanRow {
                id: "a".into(),
                desc: None,
                status: "applied".into(),
                reason: None,
                offset: Some("0x00001000".into()),
                not_applicable: false,
            },
            PlanRow {
                id: "b".into(),
                desc: None,
                status: "already".into(),
                reason: None,
                offset: None,
                not_applicable: false,
            },
            PlanRow {
                id: "c".into(),
                desc: None,
                status: "skipped".into(),
                reason: Some("当前 exe 里找不到特征".into()),
                offset: None,
                not_applicable: true,
            },
            PlanRow {
                id: "d".into(),
                desc: None,
                status: "fail".into(),
                reason: Some("期望命中 1 次，实际 0 次".into()),
                offset: None,
                not_applicable: false,
            },
        ];
        let s = summarize(&rows, 4, 10);
        assert_eq!(s.total, 10);
        assert_eq!(s.selected, 4);
        assert_eq!(s.will_apply, 2, "applied + already 都算会写进去");
        assert_eq!(s.will_skip, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.not_found, 1);
    }

    #[test]
    fn 备份命名() {
        // 第一次：没有 .bak
        assert_eq!(backup_file_name("Ragexe", false, "20250624-153012"), "Ragexe.bak");
        // 已经有 .bak：加时间戳，绝不覆盖老备份
        assert_eq!(
            backup_file_name("Ragexe", true, "20250624-153012"),
            "Ragexe.bak.20250624-153012"
        );
        // 路径拼装
        let p = backup_path(Path::new("/tmp/Ragexe.exe"), false, "20250624-153012");
        assert_eq!(p, PathBuf::from("/tmp/Ragexe.bak"));
    }

    /// 造一个最小的 PE32（够补丁引擎用：段表 + 一段可改的字节）。
    fn fake_pe() -> Vec<u8> {
        let mut d = vec![0u8; 0x400];
        d[0..2].copy_from_slice(b"MZ");
        let e_lfanew: u32 = 0x80;
        d[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        d[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84usize;
        d[coff..coff + 2].copy_from_slice(&0x014Cu16.to_le_bytes()); // machine = i386
        d[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 个段
        d[coff + 4..coff + 8].copy_from_slice(&2_025_052_929u32.to_le_bytes()); // 编译时间
        d[coff + 16..coff + 18].copy_from_slice(&0xE0u16.to_le_bytes()); // 可选头大小
        let opt = coff + 20;
        d[opt..opt + 2].copy_from_slice(&0x10Bu16.to_le_bytes()); // PE32
        d[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // 入口
        d[opt + 28..opt + 32].copy_from_slice(&0x0040_0000u32.to_le_bytes()); // 镜像基址
        let sec = opt + 0xE0;
        d[sec..sec + 8].copy_from_slice(b".text\0\0\0");
        d[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // 虚拟大小
        d[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // 虚拟地址
        d[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes()); // 原始大小
        d[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // 原始偏移
        for i in 0..16u8 {
            d[0x200 + i as usize] = i + 1;
        }
        d
    }

    #[test]
    fn 端到端预览应用校验() {
        let dir = std::env::temp_dir().join(format!(
            "warp_gui_e2e_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("client.exe");
        std::fs::write(&exe, fake_pe()).unwrap();
        let tbl = dir.join("t.yml");
        std::fs::write(
            &tbl,
            "version: 1\nmeta: {encoding: gbk}\npatches:\n  - id: t1\n    desc: 改一个字节\n    find: \"01 02 03 04 05 06 07 08\"\n    replace: \"01 02 03 04 05 06 07 09\"\n    expect: 1\n    section: .text\n",
        )
        .unwrap();
        let ids = vec!["t1".to_string()];

        // 1) 读表：特征能在 exe 里定位到
        let view = load_table(&tbl, Some(&exe)).unwrap();
        assert_eq!(view.total, 1);
        assert!(view.entries[0].available);
        assert_eq!(view.entries[0].offset.as_deref(), Some("0x00000200"));

        // 2) 预览：会打 1 条
        let p = plan(&exe, &tbl, &ids).unwrap();
        assert_eq!(p.will_apply, 1);
        assert_eq!(p.failed, 0);
        assert_eq!(p.not_found, 0);

        // 3) 应用：备份 + 写输出 + 校验通过
        let r = apply_patch(&exe, &tbl, &ids).unwrap();
        assert_eq!(r.applied, 1);
        assert!(r.verify_ok);
        assert_eq!(r.verify_passed, 1);
        assert!(Path::new(&r.backup).exists());
        assert!(Path::new(&r.out).exists());
        assert_eq!(r.out, dir.join("client_patched.exe").display().to_string());
        // 输出文件和原文件大小一样，但 md5 变了
        assert_ne!(r.out_md5, md5_file(&exe).unwrap());
        assert_eq!(r.out_size, fake_pe().len() as u64);

        // 4) 再应用一次：备份文件已存在 → 加时间戳后缀（不覆盖老备份）
        let r2 = apply_patch(&exe, &tbl, &ids).unwrap();
        assert_ne!(r2.backup, r.backup);
        assert!(r2.backup.contains("client.bak."));
        assert!(Path::new(&r.backup).exists(), "老备份不能被覆盖");
    }

    #[test]
    fn md5与输出路径() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            patched_path(Path::new("/tmp/Ragexe.exe")),
            PathBuf::from("/tmp/Ragexe_patched.exe")
        );
    }

    #[test]
    fn 解析补丁表逐条可用性() {
        let dir = temp_tables();
        let view = load_table(&dir.join("base.yml"), None).unwrap();
        assert_eq!(view.total, 2);
        assert_eq!(view.entries[0].id, "a");
        assert_eq!(view.entries[0].mode, "replace");
        // 没选客户端 exe → 可用性「未判定」
        assert!(view.entries[0].available);
        assert!(view.entries[0]
            .available_note
            .as_deref()
            .unwrap()
            .contains("未判定"));
    }
}
