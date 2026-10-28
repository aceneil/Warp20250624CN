# warp_gui —— warp_rs 的图形前端（Tauri v2）

把 `warp_rs` 的 CLI 能力（`info / find / apply / verify / langs`）包成一个中文深色桌面前端。

* **同一份前端 + 同一份 Rust 逻辑**，Windows / Linux / macOS 共用；
* 后续任意的桌面启动器按这套结构演进（逻辑层不依赖 tauri，可直接复用）。

## 界面

窗口分三块：

1. **选择目标**：客户端 exe 的路径输入框 + 「浏览…」按钮，**也支持把文件拖进窗口**
   （Tauri v2 的 `tauri://drag-drop` 事件）。选完显示文件名、大小、md5。
2. **选择补丁**：自动扫描仓库 `patches/*.yml`，下拉框列出；选一张表后解析出条目表格
   （勾选框 / id / 说明 / 模式 / 偏移 / 是否可用），默认全选。
   顶部汇总：「共 N 条，已选 M 条，其中 K 条在当前 exe 上找不到（会跳过）」。
3. **执行**：
   * 「预览」= dry-run，只算 plan，输出「会打哪些 / 跳过哪些」，**不写任何文件**；
   * 「应用」= 自动备份原 exe（`<原名>.bak`，已存在则加时间戳）→ 写 `<原名>_patched.exe`
     → 跑一次 verify，把成功 / 失败条数、输出 md5、文件大小打在日志区；
   * 「校验」= 单独校验一个已经打过补丁的 exe。

日志区可滚动、等宽字体，成功绿 / 警告黄 / 错误红；出错不 panic，弹一行中文说明。

## 目录结构

```
tools/warp_gui/
  Cargo.toml            # [lib] warp_gui + [[bin]] warp-gui
  build.rs              # 只有 Windows 目标才跑 tauri_build::build()
  tauri.conf.json       # frontendDist = ui（纯静态，无 node / vite）
  capabilities/         # 最小权限
  icons/                # 复用 updater_tauri 的图标
  src/lib.rs            # 纯逻辑：扫描 / 预览 / 备份+应用+校验（可单测，不依赖 tauri）
  src/main.rs           # Windows 起窗口；其它平台打印提示后退出（码 0）
  src/commands.rs       # Tauri command：pick_exe / scan_tables / plan / apply / verify …
  ui/                   # index.html + app.js + style.css（纯静态）
```

## 构建

```bash
cd tools/warp_gui

# Linux / macOS：只编逻辑库与命令行壳（不需要 webkit2gtk）
cargo build --release
cargo test

# Windows 目标（Linux 上交叉编译也行）
cargo xwin build --release --target x86_64-pc-windows-msvc
# → target/x86_64-pc-windows-msvc/release/warp-gui.exe
```

`tauri` 依赖放在 `[target.'cfg(target_os = "windows")'.dependencies]` 里：
Windows 目标才真的链 WebView2，其它平台只出逻辑库 / 命令行版。

## 补丁表目录怎么找

按顺序找第一个存在的：

1. 环境变量 `WARP_GUI_PATCHES_DIR`；
2. 当前工作目录 / 其上级的 `patches/`；
3. 可执行文件所在目录 / 其上级的 `patches/`。

## 已知限制

* 界面里的 `ask:` 参数（补丁表里声明的弹窗参数）目前**一律取表里的默认值**，
  没有做输入框；要改参数请先用命令行 `--set key=value`。
* macOS / Linux 只验证了逻辑层（`cargo test` / `cargo build`），
  图形窗口没有在这两个平台上实际跑过（Tauri 依赖只挂在 Windows 目标下）。
