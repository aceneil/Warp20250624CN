// warp_gui 前端：纯静态（没有 node / vite 构建链，Tauri 直接把这三个文件打进 exe）。
//
// 与后端的约定：
//   * invoke('cmd_xxx', { camelCase 参数 })  —— Tauri v2 会自动映射到 Rust 的 snake_case 形参
//   * 后端出错时 reject 一个字符串，界面上显示成一行中文说明（不 panic）
//
// 界面三块：① 选择目标 ② 选择补丁 ③ 执行。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const state = {
  exe: "",          // 选中的客户端 exe（绝对路径）
  table: "",        // 选中的补丁表文件名
  entries: [],      // 当前表的条目
  selected: new Set(), // 勾选的补丁 id
};

// ---------------------------------------------------------------------------
// 日志 / 错误
// ---------------------------------------------------------------------------

/** 往日志区写一行（kind: ok / warn / err / head）。 */
function log(text, kind) {
  const el = $("log");
  const span = document.createElement("span");
  if (kind) span.className = kind;
  span.textContent = text + "\n";
  el.appendChild(span);
  el.scrollTop = el.scrollHeight;
}

/** 弹一行中文错误说明（同时写进日志）。 */
function fail(e) {
  const msg = (e && typeof e === "object" && typeof e.message === "string")
    ? e.message
    : String(e);
  log("✘ " + msg, "err");
  const t = $("toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(fail._timer);
  fail._timer = setTimeout(() => { t.hidden = true; }, 6000);
}

/** 状态条：跑命令时禁用按钮。 */
function busy(on) {
  for (const id of ["btnBrowse", "btnPreview", "btnApply", "btnVerify"]) {
    $(id).disabled = on;
  }
}

// ---------------------------------------------------------------------------
// ① 选择目标
// ---------------------------------------------------------------------------

/** 选好客户端 exe：读信息 + 重新算补丁的可用性。 */
async function setExe(path) {
  state.exe = path;
  $("exePath").value = path;
  try {
    const info = await invoke("cmd_exe_info", { path });
    const meta = $("exeMeta");
    meta.innerHTML = "";
    const add = (k, v, cls) => {
      const a = document.createElement("span");
      a.className = "k";
      a.textContent = k + " ";
      const b = document.createElement("span");
      b.className = cls || "v";
      b.textContent = v;
      meta.appendChild(a);
      meta.appendChild(b);
      meta.appendChild(document.createElement("br"));
    };
    add("文件名", info.name);
    add("大小", info.size.toLocaleString() + " 字节");
    add("md5", info.md5);
    if (!info.isPe) {
      add("提示", "这不是 PE 文件（头两个字节不是 MZ），补丁多半打不上", "v");
    }
    log("已选择客户端：" + info.path, "ok");
    log("  文件名 " + info.name + " ｜ 大小 " + info.size.toLocaleString() + " 字节 ｜ md5 " + info.md5,
        info.isPe ? "ok" : "warn");
    // 可用性跟客户端有关 → 重新读表
    await loadTable();
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------------------
// ② 选择补丁
// ---------------------------------------------------------------------------

/** 扫描补丁表目录，填下拉框。 */
async function loadTables() {
  try {
    const res = await invoke("cmd_scan_tables");
    $("patchesDir").textContent = "补丁表目录：" + res.dir;
    const sel = $("selTable");
    sel.innerHTML = "";
    for (const t of res.tables) {
      const opt = document.createElement("option");
      opt.value = t.file;
      opt.textContent = t.error ? (t.file + "（表坏了）") : (t.file + "（" + t.count + " 条）");
      sel.appendChild(opt);
    }
    if (!res.tables.length) {
      $("tableHint").textContent = "目录里没有补丁表";
      $("tbody").innerHTML = '<tr class="empty"><td colspan="6">没有补丁表</td></tr>';
      return;
    }
    // 默认选 base.yml（没有就选第一个）
    const base = res.tables.find((t) => t.file === "base.yml");
    sel.value = base ? base.file : res.tables[0].file;
    await loadTable();
  } catch (e) {
    fail(e);
  }
}

/** 读当前选中的补丁表（逐条算可用性）。 */
async function loadTable() {
  state.table = $("selTable").value;
  if (!state.table) return;
  try {
    const view = await invoke("cmd_table_entries", {
      table: state.table,
      exe: state.exe ? state.exe : null,
    });
    state.entries = view.entries;
    state.selected = new Set(view.entries.map((e) => e.id));
    $("tableHint").textContent = "编码 " + view.encoding;
    renderTable();
  } catch (e) {
    fail(e);
  }
}

/** 画补丁表格。 */
function renderTable() {
  const tb = $("tbody");
  tb.innerHTML = "";
  if (!state.entries.length) {
    tb.innerHTML = '<tr class="empty"><td colspan="6">这张补丁表里没有条目</td></tr>';
    updateSummary();
    return;
  }
  for (const e of state.entries) {
    const tr = document.createElement("tr");
    if (!e.available) tr.className = "row-warn";

    const tdChk = document.createElement("td");
    const chk = document.createElement("input");
    chk.type = "checkbox";
    chk.checked = state.selected.has(e.id);
    chk.addEventListener("change", () => {
      if (chk.checked) state.selected.add(e.id);
      else state.selected.delete(e.id);
      updateSummary();
    });
    tdChk.appendChild(chk);
    tr.appendChild(tdChk);

    tr.appendChild(cell(e.id, "mono"));
    tr.appendChild(cell(e.desc || e.descEn || "—"));
    tr.appendChild(cell(e.mode, "mono"));
    tr.appendChild(cell(e.offset || "—", "mono dim"));

    const tdAv = document.createElement("td");
    const tag = document.createElement("span");
    tag.className = "tag " + (e.available ? "ok" : "warn");
    tag.textContent = e.available ? "可用" : "找不到";
    if (e.availableNote) tag.title = e.availableNote;
    tdAv.appendChild(tag);
    tr.appendChild(tdAv);

    tb.appendChild(tr);
  }
  updateSummary();
}

/** 一个表格单元。 */
function cell(text, cls) {
  const td = document.createElement("td");
  if (cls) td.className = cls;
  td.textContent = text;
  return td;
}

/** 顶部汇总：共 N 条 / 已选 M 条 / K 条找不到。 */
function updateSummary() {
  const total = state.entries.length;
  const m = state.selected.size;
  const k = state.entries.filter((e) => !e.available).length;
  $("summary").textContent =
    "共 " + total + " 条，已选 " + m + " 条，其中 " + k + " 条在当前 exe 上找不到（会跳过）";
  $("badgeSummary").textContent = "共 " + total + " 条 / 已选 " + m + " 条";
  $("chkAll").checked = total > 0 && m === total;
}

// ---------------------------------------------------------------------------
// ③ 执行
// ---------------------------------------------------------------------------

/** 跑命令前的前置检查。 */
function checkReady() {
  if (!state.exe) {
    fail("请先选择客户端 exe（拖进来或点「浏览…」）");
    return false;
  }
  if (!state.table) {
    fail("请先选择补丁表");
    return false;
  }
  if (!state.selected.size) {
    fail("没有勾选任何补丁");
    return false;
  }
  return true;
}

/** 预览（dry-run）。 */
async function doPreview() {
  if (!checkReady()) return;
  busy(true);
  try {
    const p = await invoke("cmd_plan", {
      exe: state.exe,
      table: state.table,
      ids: Array.from(state.selected),
    });
    log("—— 预览（dry-run，没有写任何文件）——", "head");
    log("共 " + p.total + " 条，已选 " + p.selected + " 条：" +
        "会打 " + p.willApply + " 条 / 跳过 " + p.willSkip + " 条 / 失败 " + p.failed +
        " 条 / 找不到 " + p.notFound + " 条");
    for (const r of p.rows) {
      log("  " + markOf(r.status) + " " + pad(r.id, 26) + " " + r.status +
          (r.offset ? "  @" + r.offset : "") +
          (r.reason ? "  ↳ " + r.reason : ""), kindOf(r.status));
    }
    for (const n of p.notes || []) log("  提示：" + n, "warn");
  } catch (e) {
    fail(e);
  } finally {
    busy(false);
  }
}

/** 应用：备份 → 打补丁 → 校验。 */
async function doApply() {
  if (!checkReady()) return;
  busy(true);
  try {
    const r = await invoke("cmd_apply", {
      exe: state.exe,
      table: state.table,
      ids: Array.from(state.selected),
    });
    log("—— 应用 ——", "head");
    log("已备份 → " + r.backup, "ok");
    log("已写出 → " + r.out + "（" + r.outSize.toLocaleString() + " 字节，md5 " + r.outMd5 + "）", "ok");
    log("打上 " + r.applied + " 条 / 已是目标状态 " + r.already + " 条 / 跳过 " + r.skipped +
        " 条 / 失败 " + r.failed + " 条 / 找不到 " + r.notFound + " 条");
    log("校验：通过 " + r.verifyPassed + " 条 / 失败 " + r.verifyFailed +
        " 条 / 跳过 " + r.verifySkipped + " 条", r.verifyOk ? "ok" : "err");
    for (const row of r.rows) {
      log("  " + markOf(row.status) + " " + pad(row.id, 26) + " " + row.status +
          (row.reason ? "  ↳ " + row.reason : ""), kindOf(row.status));
    }
    for (const n of r.notes || []) log("  提示：" + n, "warn");
  } catch (e) {
    fail(e);
  } finally {
    busy(false);
  }
}

/** 校验一个已经打过补丁的 exe（默认校验刚写出来的那个）。 */
async function doVerify() {
  if (!state.exe) {
    fail("请先选择客户端 exe");
    return;
  }
  busy(true);
  try {
    const r = await invoke("cmd_verify", {
      exe: state.exe,
      table: state.table,
      ids: Array.from(state.selected),
    });
    log("—— 校验 ——", "head");
    log("被校验的文件：" + r.target);
    log("共 " + r.total + " 条：通过 " + r.passed + " 条 / 失败 " + r.failed +
        " 条 / 跳过 " + r.skipped + " 条", r.ok ? "ok" : "err");
    for (const row of r.rows) {
      log("  " + markOf(row.status) + " " + pad(row.id, 26) + " " + row.status +
          (row.reason ? "  ↳ " + row.reason : ""), kindOf(row.status));
    }
  } catch (e) {
    fail(e);
  } finally {
    busy(false);
  }
}

/** 状态 → 记号。 */
function markOf(status) {
  if (status === "applied" || status === "already" || status === "ok") return "✔";
  if (status === "skipped") return "-";
  return "✘";
}

/** 状态 → 颜色。 */
function kindOf(status) {
  if (status === "applied" || status === "already" || status === "ok") return "ok";
  if (status === "skipped") return "warn";
  return "err";
}

/** id 左对齐补空格（等宽字体里好看一点）。 */
function pad(s, n) {
  s = String(s || "");
  return s.length >= n ? s : s + " ".repeat(n - s.length);
}

// ---------------------------------------------------------------------------
// 事件绑定
// ---------------------------------------------------------------------------

$("btnBrowse").addEventListener("click", async () => {
  try {
    const picked = await invoke("cmd_pick_exe");
    if (picked) await setExe(picked);
  } catch (e) {
    fail(e);
  }
});

$("btnClearExe").addEventListener("click", async () => {
  state.exe = "";
  $("exePath").value = "";
  $("exeMeta").textContent = "尚未选择客户端 exe。";
  await loadTable();
});

$("exePath").addEventListener("change", async (ev) => {
  const v = ev.target.value.trim();
  if (!v) return;
  await setExe(v);
});

$("selTable").addEventListener("change", loadTable);

$("btnAll").addEventListener("click", () => {
  state.selected = new Set(state.entries.map((e) => e.id));
  renderTable();
});

$("btnNone").addEventListener("click", () => {
  state.selected = new Set();
  renderTable();
});

$("chkAll").addEventListener("change", (ev) => {
  if (ev.target.checked) state.selected = new Set(state.entries.map((e) => e.id));
  else state.selected = new Set();
  renderTable();
});

$("btnPreview").addEventListener("click", doPreview);
$("btnApply").addEventListener("click", doApply);
$("btnVerify").addEventListener("click", doVerify);
$("btnClearLog").addEventListener("click", () => { $("log").innerHTML = ""; });

// 拖放：Tauri v2 的 `tauri://drag-drop` 事件给出绝对路径
listen("tauri://drag-drop", async (event) => {
  const paths = (event.payload && event.payload.paths) || [];
  if (paths.length) await setExe(paths[0]);
});
listen("tauri://drag-enter", () => document.body.classList.add("dragging"));
listen("tauri://drag-leave", () => document.body.classList.remove("dragging"));

// 启动
loadTables();
