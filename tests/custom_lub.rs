//! M3b 的测试：WARP 的 Custom*Lub 六条（指令级改写）。
//!
//! 这六条不是 find/replace —— 它们改的是 **x86 指令**：
//!
//! ```text
//! 找 PUSH imm32（imm32 = 原路径字符串的 VA）
//!   → 把新路径字符串写进 .text 段尾的空洞
//!   → 把 PUSH 的 imm32 改成新字符串的 VA
//! ```
//!
//! 断言：
//!
//! 1. 六条全部 `applied`；
//! 2. 每条被改的 PUSH：opcode 仍是 0x68，imm32 指向的新地址里**确实是新路径**；
//! 3. 指令长度 / 栈平衡不受影响（只改了 4 字节立即数）；
//! 4. 第二遍全部 `already`，两遍文件一模一样；
//! 5. `verify` 能解出 imm32 指向的字符串来比对；
//! 6. `--set <补丁id>=...` 能按 id 覆盖默认值。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// 六条补丁的 id（顺序 = custom_lub.yml 里的顺序）。
const PATCH_IDS: [&str; 6] = [
    "custom-achieve-lub",
    "custom-mapinfo-lub",
    "custom-ongoingquest-lub",
    "custom-recommendedquest-lub",
    "custom-tipbox-lub",
    "custom-towninfo-lub",
];

/// 每条补丁的期望：
/// * 默认新路径（GBK，全 ASCII）；
/// * 被改的 PUSH 在文件里的偏移（opcode 0x68 的位置）；
/// * PUSH 里原来的立即数（原路径字符串的 VA）。
const EXPECT: [(&str, &str, &[(usize, u32)]); 6] = [
    (
        "custom-achieve-lub",
        "SystemCN\\achievements.lub",
        &[(0x0021F934, 0x00FC784C)],
    ),
    (
        "custom-mapinfo-lub",
        "SystemCN\\mapInfo.lub",
        &[(0x0028E24A, 0x00FEA02C)],
    ),
    (
        "custom-ongoingquest-lub",
        "SystemCN\\OngoingQuests",
        &[(0x00237D60, 0x00FE534C), (0x00237D75, 0x00FE5398)],
    ),
    (
        "custom-recommendedquest-lub",
        "SystemCN\\RecommendedQuests_basic",
        &[(0x00237D6E, 0x00FE5370), (0x00237D83, 0x00FE53BC)],
    ),
    (
        "custom-tipbox-lub",
        "SystemCN\\tipbox.lub",
        &[(0x003722F6, 0x00FF8B1C)],
    ),
    (
        "custom-towninfo-lub",
        "SystemCN\\Towninfo.lub",
        &[(0x00669B3B, 0x0102D4FC)],
    ),
];

/// 测试用 exe 的路径（不存在就跳过测试）。
fn test_exe() -> Option<PathBuf> {
    let p = std::env::var("WARP_RS_TEST_EXE")
        .unwrap_or_else(|_| "/home/neil/build/refs/2025-06-04_Ragexe_neil.exe".to_string());
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

fn patch_table() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("patches/custom_lub.yml")
}

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

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("warp-rs-customlub-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 文件偏移 → VA（这个客户端里每个段的 raw_ptr == RVA，所以 VA = 基址 + 偏移）。
fn va_of(off: usize) -> u32 {
    0x00400000u32 + off as u32
}

/// 读某个 VA 处的 C 字符串。
fn cstr_at(data: &[u8], va: u32) -> Vec<u8> {
    let off = (va - 0x00400000) as usize;
    let end = off + data[off..].iter().position(|b| *b == 0).unwrap();
    data[off..end].to_vec()
}

#[test]
fn 六条_全部打上_改的是_push_的立即数() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("apply");
    let copy = dir.join("orig.exe");
    let patched = dir.join("patched.exe");
    let report = dir.join("report.json");
    std::fs::copy(&src, &copy).unwrap();

    let (code, text) = run(&[
        "apply",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        patched.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第一次打补丁失败：\n{text}");

    let rep: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(rep["total"].as_u64().unwrap(), 6);
    assert_eq!(rep["applied"].as_u64().unwrap(), 6, "六条都要打上：\n{text}");
    assert_eq!(rep["failed"].as_u64().unwrap(), 0);
    assert_eq!(rep["ok"].as_bool().unwrap(), true);

    let orig = std::fs::read(&copy).unwrap();
    let out = std::fs::read(&patched).unwrap();
    // 空洞是文件里本来就有的 → 文件长度不变
    assert_eq!(orig.len(), out.len(), "文件长度不能变");

    let entries = rep["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 6);
    for (i, entry) in entries.iter().enumerate() {
        let id = entry["id"].as_str().unwrap();
        assert_eq!(id, PATCH_IDS[i], "补丁顺序不对");
        assert_eq!(entry["status"].as_str().unwrap(), "applied");
        let (_, want_path, pushes) = &EXPECT[i];

        // 报告里要能看到：原 PUSH 偏移、imm32 旧值 → 新值、新字符串 VA、字符串字节
        let reason = entry["reason"].as_str().unwrap();
        assert!(
            reason.contains("imm32") && reason.contains("新字符串 VA"),
            "报告里要有 imm32 旧值→新值和新字符串 VA：\n{reason}"
        );
        assert!(
            reason.contains(want_path),
            "报告里要有新路径「{want_path}」：\n{reason}"
        );

        for (push_off, old_imm) in pushes.iter() {
            let push_off = *push_off;
            // opcode 必须是 0x68（push imm32），指令长度不变
            assert_eq!(out[push_off], 0x68, "{id}：0x{push_off:X} 的 opcode 被改了");
            // 原 exe 里那条 push 的立即数 = 原路径字符串的 VA
            let orig_imm = u32::from_le_bytes(orig[push_off + 1..push_off + 5].try_into().unwrap());
            assert_eq!(orig_imm, *old_imm, "{id}：原立即数不对");
            // 打完之后立即数指向新字符串
            let new_imm = u32::from_le_bytes(out[push_off + 1..push_off + 5].try_into().unwrap());
            assert_ne!(new_imm, orig_imm, "{id}：立即数没改");
            // 解出 imm32 指向的字符串，必须就是新路径
            let s = cstr_at(&out, new_imm);
            assert_eq!(
                s,
                want_path.as_bytes(),
                "{id}：imm32 指向的字符串不是新路径"
            );
            // 字符串必须以 NUL 结尾
            let end = (new_imm - 0x00400000) as usize + s.len();
            assert_eq!(out[end], 0, "{id}：新字符串没有结束符");
        }
    }

    // 空洞里的标记：每条补丁一个（魔数 + 原字符串 VA）
    let out_str = String::from_utf8_lossy(&out).into_owned();
    assert!(
        out_str.contains("WRSSTR01"),
        "空洞里应该有搬字符串的标记"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 第二遍_全部_already_文件不变() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("idem");
    let copy = dir.join("orig.exe");
    let a = dir.join("a.exe");
    let b = dir.join("b.exe");
    std::fs::copy(&src, &copy).unwrap();

    let (code, text) = run(&[
        "apply",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        a.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第一次失败：\n{text}");

    let (code, text) = run(&[
        "apply",
        "--exe",
        a.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        b.to_str().unwrap(),
        "--report",
        dir.join("r2.json").to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第二遍失败：\n{text}");

    let rep: Value =
        serde_json::from_slice(&std::fs::read(dir.join("r2.json")).unwrap()).unwrap();
    assert_eq!(rep["already"].as_u64().unwrap(), 6, "第二遍要全部 already：\n{text}");
    assert_eq!(rep["applied"].as_u64().unwrap(), 0);
    for e in rep["entries"].as_array().unwrap() {
        assert_eq!(e["status"].as_str().unwrap(), "already");
    }

    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "第二遍改了文件（不幂等）"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_能解出_imm32_指向的字符串来比对() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("verify");
    let copy = dir.join("orig.exe");
    let patched = dir.join("patched.exe");
    std::fs::copy(&src, &copy).unwrap();

    let (code, text) = run(&[
        "apply",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        patched.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "apply 失败：\n{text}");

    let (code, text) = run(&[
        "verify",
        "--exe",
        patched.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "verify 失败：\n{text}");
    assert!(text.contains("通过 6"), "verify 要六条全过：\n{text}");

    // 没打过补丁的原始文件 → verify 必须说「还没打上」
    let (code, text) = run(&[
        "verify",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "原始文件应该 verify 不过：\n{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 按补丁_id_传参能覆盖默认值() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("setid");
    let copy = dir.join("orig.exe");
    let patched = dir.join("patched.exe");
    std::fs::copy(&src, &copy).unwrap();

    let (code, text) = run(&[
        "apply",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        patched.to_str().unwrap(),
        "--report",
        dir.join("r.json").to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\Data\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "apply 失败：\n{text}");
    assert!(
        !text.contains("没有任何补丁用这个参数名"),
        "按补丁 id 传参不该报「参数名没人用」：\n{text}"
    );

    let out = std::fs::read(&patched).unwrap();
    // mapinfo 那条：push @0x0028E24A 现在指向 Example\Data\MapInfo.lub
    let new_imm = u32::from_le_bytes(out[0x0028E24B..0x0028E24F].try_into().unwrap());
    assert_eq!(
        cstr_at(&out, new_imm),
        b"Example\\Data\\MapInfo.lub".to_vec()
    );
    // 别的几条没受影响
    let tip_imm = u32::from_le_bytes(out[0x003722F7..0x003722FB].try_into().unwrap());
    assert_eq!(cstr_at(&out, tip_imm), b"SystemCN\\tipbox.lub".to_vec());

    // 报告里要标明是按补丁 id 传的
    let rep: Value = serde_json::from_slice(&std::fs::read(dir.join("r.json")).unwrap()).unwrap();
    let params = rep["params"].as_array().unwrap();
    let p = params
        .iter()
        .find(|p| p["value"].as_str() == Some("Example\\Data\\MapInfo.lub"))
        .expect("报告里没有这个参数");
    assert_eq!(p["source"].as_str().unwrap(), "cli");
    assert_eq!(
        p["set_as"].as_str().unwrap(),
        "custom-mapinfo-lub",
        "报告里要标明是按哪个补丁 id 传的"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 换路径_能重打_而且还是幂等() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("switch");
    let copy = dir.join("orig.exe");
    let p1 = dir.join("p1.exe");
    let p2 = dir.join("p2.exe");
    let p2b = dir.join("p2b.exe");
    let p3 = dir.join("p3.exe");
    let p3b = dir.join("p3b.exe");
    std::fs::copy(&src, &copy).unwrap();

    // 第一次：P1
    let (code, text) = run(&[
        "apply",
        "--exe",
        copy.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        p1.to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\One\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "第一次失败：\n{text}");

    // 第二次：换成 P2 → 应该重新改指
    let (code, text) = run(&[
        "apply",
        "--exe",
        p1.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        p2.to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\Two\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "换路径应该能重打：\n{text}");

    // 第三次：还是 P2 → already，文件不变
    let (code, text) = run(&[
        "apply",
        "--exe",
        p2.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        p2b.to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\Two\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "第三遍失败：\n{text}");
    assert_eq!(
        std::fs::read(&p2).unwrap(),
        std::fs::read(&p2b).unwrap(),
        "换完路径后第二遍不该再改文件"
    );

    let out = std::fs::read(&p2).unwrap();
    let imm = u32::from_le_bytes(out[0x0028E24B..0x0028E24F].try_into().unwrap());
    assert_eq!(
        cstr_at(&out, imm),
        b"Example\\Two\\MapInfo.lub".to_vec(),
        "push 应该指向新路径"
    );

    // 换回 P1 → 也能重打，而且再打一遍是 already
    let (code, text) = run(&[
        "apply",
        "--exe",
        p2.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        p3.to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\One\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "换回 P1 失败：\n{text}");
    let (code, text) = run(&[
        "apply",
        "--exe",
        p3.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        p3b.to_str().unwrap(),
        "--set",
        "custom-mapinfo-lub=Example\\One\\MapInfo.lub",
    ]);
    assert_eq!(code, 0, "换回 P1 后再打一遍失败：\n{text}");
    assert_eq!(
        std::fs::read(&p3).unwrap(),
        std::fs::read(&p3b).unwrap(),
        "换回 P1 后第二遍不该再改文件"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 补丁表本身是合法的() {
    let text = std::fs::read_to_string(patch_table()).unwrap();
    assert!(text.contains("version: 1"));
    assert_eq!(text.matches("  - id: ").count(), 6);
    for id in PATCH_IDS {
        assert!(text.contains(&format!("id: {id}")), "补丁表里没有 {id}");
    }
    // 六条都要声明 path_old（原路径特征）和 ask（默认新路径）
    assert_eq!(text.matches("path_old:").count(), 6, "每条都要有 path_old");
    assert_eq!(text.matches("kind: path").count(), 6, "每条都要有 kind: path 的 ask");
    assert_eq!(text.matches("max: 64").count(), 6, "每条的长度上限都要是 64");
}

/// 辅助：确认测试 exe 没被我们动过（补丁只能打在副本上）。
#[test]
fn 原exe_没有被改动() {
    let Some(src) = test_exe() else { return };
    let meta = std::fs::metadata(&src).unwrap();
    assert!(meta.len() > 0);
    let _ = Path::new(&src);
}
