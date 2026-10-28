//! 集成测试：拿真实客户端 exe 跑一遍「打补丁 → 再打一次 → 校验」。
//!
//! 测试用的 exe 用环境变量 `WARP_RS_TEST_EXE` 指定，
//! 未设置就跳过（仓库里不放客户端 exe）。
//! **文件不存在就跳过测试**（仓库里不放 14MB 的 exe）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 测试用 exe 的路径（不存在就跳过测试）。
fn test_exe() -> Option<PathBuf> {
    let p = match std::env::var("WARP_RS_TEST_EXE") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("跳过：未设置 WARP_RS_TEST_EXE（指向一个真实的客户端 exe）");
            return None;
        }
    };
    let p = PathBuf::from(p);
    if p.exists() {
        Some(p)
    } else {
        eprintln!(
            "跳过：测试用 exe 不存在（{}）。用 WARP_RS_TEST_EXE 指定。",
            p.display()
        );
        None
    }
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

/// 把文件复制到临时目录，返回临时路径。
fn temp_copy(src: &Path, tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("warp-rs-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("client.exe");
    std::fs::copy(src, &dst).unwrap();
    dst
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("warp-rs-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// 「메시지 → 消息」这条补丁（text 写法）。
const PATCH_TEXT: &str = r#"
version: 1
patches:
  - id: translate_msgbox
    desc: 消息框标题
    find: "B8 DE BD C3 C1 F6 00"
    text: "消息"
    encoding: gbk
    section: .rdata
    expect: 1
    mode: replace
    dates: { start: 20100000, end: 20991231 }
"#;

/// 同一条补丁的 hex 写法。
const PATCH_HEX: &str = r#"
version: 1
patches:
  - id: translate_msgbox
    desc: 消息框标题
    find: "B8 DE BD C3 C1 F6 00"
    replace: "CF FB CF A2"
    terminate: true
    section: .rdata
    expect: 1
    mode: replace
    dates: { start: 20100000, end: 20991231 }
"#;

#[test]
fn 打上消息框翻译_字节变成_消息() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("msgbox");
    let src = temp_copy(&exe, "msgbox");
    let yml = dir.join("patch.yml");
    std::fs::write(&yml, PATCH_TEXT).unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");
    assert!(out.exists(), "输出文件应该存在");

    // 找到原来「메시지」的位置，确认那里现在是 GBK 的「消息」+ 结束符
    let before = std::fs::read(&src).unwrap();
    let after = std::fs::read(&out).unwrap();
    assert_eq!(before.len(), after.len(), "原地覆写，文件长度不能变");

    let pat = [0xB8u8, 0xDE, 0xBD, 0xC3, 0xC1, 0xF6, 0x00];
    let pos = before
        .windows(pat.len())
        .position(|w| w == pat)
        .expect("原文件里应该能找到「메시지」");
    assert_eq!(&after[pos..pos + 5], &[0xCF, 0xFB, 0xCF, 0xA2, 0x00]);
    // 除了这一处，其它字节都不能动
    let mut diff = 0;
    for i in 0..before.len() {
        if before[i] != after[i] {
            diff += 1;
        }
    }
    assert_eq!(diff, 5, "只应该改动 5 个字节");

    cleanup(&dir);
}

#[test]
fn hex写法_结果一致() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("msgboxhex");
    let src = temp_copy(&exe, "msgboxhex");
    let yml = dir.join("patch.yml");
    std::fs::write(&yml, PATCH_HEX).unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");

    let before = std::fs::read(&src).unwrap();
    let after = std::fs::read(&out).unwrap();
    let pat = [0xB8u8, 0xDE, 0xBD, 0xC3, 0xC1, 0xF6, 0x00];
    let pos = before
        .windows(pat.len())
        .position(|w| w == pat)
        .expect("原文件里应该能找到「메시지」");
    assert_eq!(&after[pos..pos + 5], &[0xCF, 0xFB, 0xCF, 0xA2, 0x00]);

    cleanup(&dir);
}

#[test]
fn 再打一次_状态全是_already() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("idem");
    let src = temp_copy(&exe, "idem");
    let yml = dir.join("patch.yml");
    std::fs::write(&yml, PATCH_TEXT).unwrap();
    let out1 = dir.join("patched1.exe");
    let out2 = dir.join("patched2.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out1.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第一次 apply 应该成功：\n{text}");
    assert!(text.contains("applied"), "第一次应该是 applied：\n{text}");

    // 第二次：对同一个输出再打一遍
    let (code, text) = run(&[
        "apply",
        "--exe",
        out1.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out2.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第二次 apply 应该成功：\n{text}");
    assert!(text.contains("already"), "第二次应该是 already：\n{text}");
    assert!(!text.contains("fail"), "第二次不该有失败：\n{text}");

    // 两次打出来的文件应该一模一样
    assert_eq!(
        std::fs::read(&out1).unwrap(),
        std::fs::read(&out2).unwrap(),
        "幂等：第二次不应该再改任何字节"
    );

    cleanup(&dir);
}

#[test]
fn verify_退出码为0() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("verify");
    let src = temp_copy(&exe, "verify");
    let yml = dir.join("patch.yml");
    std::fs::write(&yml, PATCH_TEXT).unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "apply 应该成功：\n{text}");

    let (code, text) = run(&[
        "verify",
        "--exe",
        out.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "verify 应该通过：\n{text}");
    assert!(text.contains("ok"), "verify 应该报 ok：\n{text}");

    cleanup(&dir);
}

#[test]
fn 命中次数不符_整份不落盘() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("expect");
    let src = temp_copy(&exe, "expect");
    let yml = dir.join("patch.yml");
    // expect 写成 3，实际只有 1 处命中 → 这条必须失败
    std::fs::write(
        &yml,
        r#"
version: 1
patches:
  - id: translate_msgbox
    find: "B8 DE BD C3 C1 F6 00"
    replace: "CF FB CF A2"
    terminate: true
    section: .rdata
    expect: 3
    mode: replace
"#,
    )
    .unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "应该失败：\n{text}");
    assert!(text.contains("fail"), "应该有 fail：\n{text}");
    assert!(!out.exists(), "有失败时整份补丁不该落盘");

    cleanup(&dir);
}

#[test]
fn dry_run_不写文件() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("dryrun");
    let src = temp_copy(&exe, "dryrun");
    let yml = dir.join("patch.yml");
    std::fs::write(&yml, PATCH_TEXT).unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code, 0, "dry-run 应该成功：\n{text}");
    assert!(!out.exists(), "dry-run 不该写输出文件");
    assert!(text.contains("applied"), "dry-run 也要报告结果：\n{text}");

    cleanup(&dir);
}

#[test]
fn 替换过长_跳过不写坏文件() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("toolong");
    let src = temp_copy(&exe, "toolong");
    let yml = dir.join("patch.yml");
    // 替换 8 字节 > find 4 字节 → 必须拒绝
    std::fs::write(
        &yml,
        r#"
version: 1
patches:
  - id: too_long
    find: "B8 DE BD C3"
    replace: "CF FB CF A2 CF FB CF A2"
    section: .rdata
    expect: 1
    mode: replace
"#,
    )
    .unwrap();
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "替换过长应该失败：\n{text}");
    assert!(!out.exists(), "不该落盘");
    // 原文件没被动过
    assert_eq!(
        std::fs::read(&src).unwrap().len(),
        std::fs::metadata(&exe).unwrap().len() as usize
    );

    cleanup(&dir);
}

#[test]
fn 完整中文表_能打完且能校验() {
    let Some(exe) = test_exe() else { return };
    let dir = temp_dir("full");
    let src = temp_copy(&exe, "full");
    let yml = Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/lang/zh-cn.yml");
    let out = dir.join("patched.exe");

    let (code, text) = run(&[
        "apply",
        "--exe",
        src.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "整套中文补丁应该打得完：\n{text}");
    assert!(!text.contains("fail"), "不该有失败：\n{text}");

    let (code, text) = run(&[
        "verify",
        "--exe",
        out.to_str().unwrap(),
        "--patches",
        yml.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "verify 应该通过：\n{text}");

    cleanup(&dir);
}
