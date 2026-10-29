# Warp20250624CN

[English](README.md) | **中文**

**WARP 补丁引擎的 Rust 重写**，面向 32 位仙境传说（Ragnarok Online）客户端：
按声明式补丁表在 exe 里定位特征字节并原地改写。

WARP（Neo-Mind/WARP，GPL-3.0）是一个 Qt 界面 + 一批 `.qjs` 脚本，做的正是
「找特征串 → 覆盖字节」。本项目用 Rust 重新实现这套引擎，因此可以嵌进启动器、
用命令行驱动、并且不依赖 Qt 就能交叉编译。

> 独立实现。只参考了 WARP 公开的补丁行为，没有复制任何 C++ / `.qjs` 源码。

## 能力

| | |
|---|---|
| PE32 / PE32+ 解析 | 段表、编译时间戳、VA ↔ 文件偏移、加壳特征 |
| 特征搜索 | hex（支持 `??` 通配）、文本（cp949 / gbk / utf8 / sjis / cp874）、`nth` / `last` / 全部命中、限定段 |
| 写入原语 | `SetHex`、`SetU8/16/32`、NOP 填充、相对 JMP、按固定偏移写 |
| 声明式补丁表 | YAML → `apply` / `verify`，输出 JSON 报告 |
| 安全 | 严格边界检查、幂等（识别已打过）、任一条失败则整份不落盘 |
| offset 模式 | 固定偏移写入 + 原字节校验（用于代码洞、PE 头字段这类特征串定位不到的位置）|
| 可作库使用 | `warp_rs` 同时是 lib 和 bin，启动器可直接依赖 |

## 安装

### 桌面程序（推荐）

每个 tag 的 Release 只挂 **图形界面** 产物（Tauri v2，一套代码两端共用）：

| 平台 | 文件 | 运行依赖 |
|---|---|---|
| Linux x86-64 | `warp-gui-linux-x86_64.tar.gz` | `webkit2gtk-4.1` |
| Windows x86-64 | `warp-gui-windows-x86_64.zip` | WebView2（Win10/11 自带） |

选客户端 exe → 勾选补丁 → 预览 → 应用。应用前自动备份为 `<原名>.bak`，写完自动校验。
补丁表来自程序旁边的 `patches/*.yml`，不依赖任何远端 profile。

```bash
# 自己构建 GUI
cargo build --release --features gui --manifest-path tools/warp_gui/Cargo.toml
```

### 命令行

```bash
cargo build --release            # → target/release/warp_rs
```

在 Linux 上交叉编译 Windows 版需要 [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin)：

```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```

## 用法

```bash
# 看目标 exe 的 PE 信息
warp_rs info --exe Ragexe.exe

# 搜特征串（hex 支持通配，也可以搜文本）
warp_rs find --exe Ragexe.exe --hex "B8 DE BD C3 C1 F6"

# 按补丁表打补丁，输出新文件 + JSON 报告
warp_rs apply --exe Ragexe.exe --patches patches/zh-cn.yml \
        --out Ragexe.patched.exe --report report.json

# 校验结果
warp_rs verify --exe Ragexe.patched.exe --patches patches/zh-cn.yml

# 列出语言包（给启动器下拉框用）
warp_rs langs --dir patches/lang
```

## 补丁表

```yaml
version: 1
patches:
  - id: example_string
    desc: 示例字符串替换
    find: "B8 DE BD C3 C1 F6 00"
    replace: "CF FB CF A2 00"   # 按客户端代码页给出的原始字节
    section: .rdata             # 可选：限定搜索范围
    expect: 1                   # 期望命中次数
    mode: replace               # replace | nop | jmp | offset
```

要点：

- 替换不能比原特征串更长；**唯一例外**是可以借用匹配之后紧跟的连续 `0x00` 填充
  （只覆盖 NUL，碰到非零字节就停）。
- 非 ASCII 文本一律用十六进制按客户端代码页给出，绕开工具链上的所有编码问题。

### offset 模式

有些位置用特征串定位不到（代码洞、全局指针、PE 头字段），可以直接给偏移并校验原字节：

```yaml
  - id: parity-0042
    mode: offset
    offset:  "0x0095B1DC"
    verify:  "A1 00 46 57 01"     # 原字节不符就拒打（版本守卫）
    replace: "E9 10 00 00 00"
```

### 复现一份已知可用的产物

当某个参考实现的产出确认可用、而语义化补丁表复现不出来时，直接对两份 exe 做 diff
生成 offset 表。打出来的文件与参考产物**逐字节相同**：

```bash
scripts/gen-parity-table.sh pristine.exe reference.exe patches/parity.yml
warp_rs apply --exe pristine.exe --patches patches/parity.yml --out out.exe
md5sum out.exe reference.exe      # 相等
```

## 范围

- 只用于操作者有权修改的可执行文件；仓库不附带也不分发任何游戏客户端。
- 测试用的 exe 放在仓库外，通过环境变量 `WARP_RS_TEST_EXE` 指定。
- 目前命令行输出是中文，英文输出在路线图里。

## 许可

GPL-3.0 —— 见 [LICENSE](LICENSE)。`patches/` 下的补丁数据源自 GPL-3.0 的 WARP 翻译表，
因此整个项目按同一许可发布。

## 路线图

见 [docs/ROADMAP.md](docs/ROADMAP.md)。
