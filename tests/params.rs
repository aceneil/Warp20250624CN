//! M3a 的测试：补丁表参数（`ask:`）、写入编码、语言包。
//!
//! 测试用的 exe 用环境变量 `WARP_RS_TEST_EXE` 指定，
//! 缺省 `/home/neil/build/refs/2025-06-04_Ragexe_neil.exe`。
//! **文件不存在就跳过测试**（仓库里不放 15MB 的 exe）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// 测试用 exe 的路径（不存在就跳过测试）。
fn test_exe() -> Option<PathBuf> {
    let p = std::env::var("WARP_RS_TEST_EXE")
        .unwrap_or_else(|_| "/home/neil/build/refs/2025-06-04_Ragexe_neil.exe".to_string());
    let p = PathBuf::from(p);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("跳过：测试用 exe 不存在（{}）。", p.display());
        None
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 跑 warp_rs，返回 (退出码, stdout+stderr)。
fn run(args: &[&str]) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_warp_rs");
    let out = Command::new(exe)
        .args(args)
        .output()
        .expect("跑 warp_rs 失败");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

/// 跑 warp_rs，喂一段 stdin（测 --interactive）。
fn run_with_stdin(args: &[&str], input: &str) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_warp_rs");
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("跑 warp_rs 失败");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("warp-rs-params-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 带 ask 参数的补丁表。
fn ask_table(dir: &Path, default: Option<&str>, extra: &str) -> PathBuf {
    let mut yml = String::from(
        r#"version: 1
patches:
  - id: window_title
    desc: "自定义窗口标题"
    ask:
      key: title
      label: "请输入新的窗口标题"
      kind: text
      min: 1
      max: 60
"#,
    );
    if let Some(d) = default {
        yml.push_str(&format!("      default: \"{d}\"\n"));
    }
    yml.push_str(
        r#"    find: "52 61 67 6E 61 72 6F 6B 00 00 00 00"
    replace: "{{title}}"
    terminate: true
    zero_fill: true
    section: .rdata
    expect: 1
    mode: replace
"#,
    );
    yml.push_str(extra);
    let path = dir.join("ask.yml");
    std::fs::write(&path, yml).unwrap();
    path
}

/// 窗口标题那条补丁在 exe 里的落点（.rdata 里独立的 "Ragnarok"）。
const TITLE_OFFSET: usize = 0x00BF2604;

#[test]
fn set_参数_按_gbk_写进_exe() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("set");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        "title=中文标题",
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");

    let data = std::fs::read(&out).unwrap();
    // 「中文标题」的 GBK 字节 + 结束符
    assert_eq!(
        &data[TITLE_OFFSET..TITLE_OFFSET + 9],
        &[0xD6, 0xD0, 0xCE, 0xC4, 0xB1, 0xEA, 0xCC, 0xE2, 0x00]
    );
    // 剩下的匹配区被清零（zero_fill）
    assert_eq!(&data[TITLE_OFFSET + 9..TITLE_OFFSET + 12], &[0u8; 3]);
    // 别的字节没动
    let orig = std::fs::read(&exe).unwrap();
    assert_eq!(orig.len(), data.len());
    let diff: Vec<usize> = (0..data.len()).filter(|&i| data[i] != orig[i]).collect();
    assert_eq!(diff.len(), 8, "只应该改窗口标题那 8 个字符");
    assert_eq!(diff[0], TITLE_OFFSET);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 不给参数_又没默认值_必须报错() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("noparams");
    let yml = ask_table(&dir, None, "");
    let out = dir.join("out.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "应该报错：\n{text}");
    assert!(text.contains("需要参数 title"), "报错要说清楚缺哪个参数：\n{text}");
    assert!(!out.exists(), "不该落盘");

    let _ = std::fs::remove_dir_all(&dir);
}

// M3b 起：标题放不下 11 字节的原地预算时，改成「搬进 .text 段尾的空洞」
// （和 Custom*Lub 六条同一套机制）。所以这里不再是「报错」，
// 而是「搬过去，而且一个字符都不能少」。
#[test]
fn 超长标题_搬到空洞_不能截断() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("toolong");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");
    let long = "长".repeat(40); // GBK 80 字节，原地预算只有 11 字节

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        &format!("title={long}"),
    ]);
    assert_eq!(code, 0, "长标题应该搬到空洞里：\n{text}");
    assert!(text.contains("搬进空洞"), "报告要说清楚是「搬」不是「原地写」：\n{text}");

    let data = std::fs::read(&out).unwrap();
    // 原来的 11 字节预算区一个字节都没动（还是 "Ragnarok"）
    assert_eq!(&data[TITLE_OFFSET..TITLE_OFFSET + 9], b"Ragnarok\0");

    // 80 字节的标题完整地躺在文件里（没有被截断），后面紧跟结束符
    let mut needle = encoding_rs::GBK.encode(&long).0.to_vec();
    needle.push(0);
    let at = data
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .expect("文件里找不到完整的标题");
    assert_eq!(&data[at..at + needle.len()], needle.as_slice());

    // 引用标题的两条指令都改指到新字符串了：
    //   0x00333363: B8 <imm32>                    mov eax, 0x00FF2604
    //   0x00A59DD0: C7 05 80 D8 1E 01 <imm32>    mov [0x011ED880], 0x00FF2604
    // 这个客户端里每个段的 raw_ptr == RVA，所以 文件偏移 + 镜像基址 = VA
    let va = 0x00400000u32 + at as u32;
    for (off, len) in [(0x00333364usize, 4usize), (0x00659DD6usize, 4usize)] {
        let imm = u32::from_le_bytes(data[off..off + len].try_into().unwrap());
        assert_eq!(imm, va, "0x{off:X} 的立即数应该指向新标题");
    }

    // verify 要认得出来（会解出 imm32 指向的字符串来比对）
    let (code, text_v) = run(&[
        "verify",
        "--exe",
        out.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--set",
        &format!("title={long}"),
    ]);
    assert_eq!(code, 0, "verify 应该通过：\n{text_v}");

    // 再打一遍 → 幂等，文件不变
    let out2 = dir.join("out2.exe");
    let (code, text2) = run(&[
        "apply",
        "--exe",
        out.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out2.to_str().unwrap(),
        "--set",
        &format!("title={long}"),
    ]);
    assert_eq!(code, 0, "第二遍应该成功：\n{text2}");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(&out2).unwrap(),
        "第二遍不该改文件（不幂等）"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// 连 .text 段尾的空洞都放不下 → 必须明确报错，不能截断
#[test]
fn 标题大到连空洞都放不下_必须报错() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("waytoolong");
    // max 放到 5000，让标题能长到把空洞撑爆
    let mut yml = ask_table(&dir, Some("Ragnarok"), "");
    let text = std::fs::read_to_string(&yml).unwrap().replace("max: 60", "max: 5000");
    std::fs::write(&yml, text).unwrap();
    let out = dir.join("out.exe");
    let long = "长".repeat(3000); // GBK 6000 字节

    let (code, text_out) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        &format!("title={long}"),
    ]);
    assert_eq!(code, 1, "放不下应该报错：\n{text_out}");
    assert!(
        text_out.contains("空洞") || text_out.contains("太长"),
        "报错要说清楚放不下：\n{text_out}"
    );
    assert!(!out.exists(), "不该落盘");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 交互式_能拿到用户输入() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("interactive");
    let yml = ask_table(&dir, None, "");
    let out = dir.join("out.exe");

    let (code, text) = run_with_stdin(
        &[
            "apply",
            "--exe",
            exe.to_str().unwrap(),
            "--patches",
            yml.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--interactive",
        ],
        "中文标题\n",
    );
    assert_eq!(code, 0, "交互式应该成功：\n{text}");
    assert!(text.contains("请输入新的窗口标题"), "提示要显示 label：\n{text}");

    let data = std::fs::read(&out).unwrap();
    assert_eq!(
        &data[TITLE_OFFSET..TITLE_OFFSET + 9],
        &[0xD6, 0xD0, 0xCE, 0xC4, 0xB1, 0xEA, 0xCC, 0xE2, 0x00]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 默认值_在报告里标明来源() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("default");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");
    let report = dir.join("report.json");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");
    let rep: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(rep["encoding"].as_str().unwrap(), "gbk");
    let params = rep["params"].as_array().unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["key"].as_str().unwrap(), "title");
    assert_eq!(params[0]["value"].as_str().unwrap(), "Ragnarok");
    assert_eq!(params[0]["source"].as_str().unwrap(), "default");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn encoding_utf8_按_utf8_写字节() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("utf8");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");
    let report = dir.join("report.json");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--encoding",
        "utf8",
        "--set",
        "title=中文",
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");
    let data = std::fs::read(&out).unwrap();
    // UTF-8 的「中文」= 6 字节
    assert_eq!(
        &data[TITLE_OFFSET..TITLE_OFFSET + 7],
        &[0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87, 0x00]
    );
    let rep: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(rep["encoding"].as_str().unwrap(), "utf8");
    assert_eq!(rep["entries"][0]["encoding_source"].as_str().unwrap(), "cli");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gbk_表示不了的字符_要报错并指出是哪个字符() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("badchar");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        "title=😀",
    ]);
    assert_eq!(code, 1, "应该报错：\n{text}");
    assert!(
        text.contains("参数 title 的文本「😀」无法用 GBK 表示"),
        "报错格式要固定：\n{text}"
    );
    assert!(!out.exists(), "不该落盘");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_要用和_apply_一样的编码() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("verifyenc");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        "title=中文标题",
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");

    // 同样的参数 + 同样的编码 → verify 通过
    let (code, text) = run(&[
        "verify",
        "--exe",
        out.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--set",
        "title=中文标题",
    ]);
    assert_eq!(code, 0, "verify 应该通过：\n{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 没有_meta_只是警告_不是错误() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("nometa");
    let yml = ask_table(&dir, Some("Ragnarok"), "");
    let out = dir.join("out.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "没有 meta 也应该能打：\n{text}");
    assert!(text.contains("没有 meta"), "应该给个警告：\n{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dry_run_报告里写清楚有几条不适用() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("dryrun");
    let yml = dir.join("nope.yml");
    std::fs::write(
        &yml,
        r#"version: 1
patches:
  - id: not_in_this_client
    desc: "这个客户端没有的特征"
    find: "DE AD BE EF 00 11 22 33"
    replace: "FF EE DD CC"
    expect: 1
    mode: replace
"#,
    )
    .unwrap();
    let report = dir.join("report.json");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        dir.join("out.exe").to_str().unwrap(),
        "--dry-run",
        "--report",
        report.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "找不到特征应该失败：\n{text}");
    let rep: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(rep["not_applicable"].as_u64().unwrap(), 1);
    let notes = rep["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n.as_str().unwrap().contains("1 条不适用")),
        "报告里要写清楚有几条不适用：{notes:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn langs_能列出语言包() {
    let dir = manifest_dir().join("patches/lang");
    let (code, text) = run(&["langs", "--dir", dir.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0, "langs 应该成功：\n{text}");
    let list: Value = serde_json::from_str(&text).unwrap();
    let arr = list.as_array().unwrap();
    assert!(arr.len() >= 2, "至少要有 zh-cn 和 en：{text}");
    let zh = arr
        .iter()
        .find(|l| l["lang"].as_str() == Some("zh-cn"))
        .expect("没找到 zh-cn");
    assert_eq!(zh["name"].as_str().unwrap(), "简体中文");
    assert_eq!(zh["encoding"].as_str().unwrap(), "gbk");
    assert!(zh["count"].as_u64().unwrap() >= 30);
}

#[test]
fn base_表里的窗口标题补丁能打上() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("basetitle");
    let yml = manifest_dir().join("patches/base.yml");
    let out = dir.join("out.exe");
    let report = dir.join("report.json");

    let (code, text) = run(&[
        "apply",
        "--exe",
        exe.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--set",
        "title=中文标题",
    ]);
    assert_eq!(code, 0, "base.yml 应该打得完：\n{text}");
    let rep: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(rep["failed"].as_u64().unwrap(), 0);
    let entry = rep["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"].as_str() == Some("window_title"))
        .expect("报告里没有 window_title");
    assert_eq!(entry["offset"].as_u64().unwrap() as usize, TITLE_OFFSET);
    assert_eq!(entry["params"][0]["value"].as_str().unwrap(), "中文标题");
    assert_eq!(entry["params"][0]["source"].as_str().unwrap(), "cli");

    let data = std::fs::read(&out).unwrap();
    assert_eq!(
        &data[TITLE_OFFSET..TITLE_OFFSET + 9],
        &[0xD6, 0xD0, 0xCE, 0xC4, 0xB1, 0xEA, 0xCC, 0xE2, 0x00]
    );

    let _ = std::fs::remove_dir_all(&dir);
}
