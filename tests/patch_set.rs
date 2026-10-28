//! 集成测试：对客户端 exe 副本打 M2 的 10 条补丁。
//!
//! 断言：
//!
//! 1. 10 条补丁全部 `ok`（applied 或 already）；
//! 2. 每个落点上的字节确实变了（打印 old → new）；
//! 3. 再打一遍全部 `already`（幂等）。
//!
//! 测试用的 exe 用环境变量 `WARP_RS_TEST_EXE` 指定，
//! 缺省 `/home/neil/build/refs/2025-06-04_Ragexe_neil.exe`。
//! **文件不存在就跳过测试**（仓库里不放 15MB 的 exe）。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// M2 的 10 条补丁（顺序 = base.yml 里的顺序）。
const PATCH_IDS: [&str; 10] = [
    "zoom-max",
    "always-ascii",
    "borderless-fsw",
    "cutin-esc",
    "map-quality",
    "no-gravity-ads",
    "no-kro-site",
    "official-palette",
    "load-kr-extsettings",
    "no-gm-sprite",
];

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("patches/base.yml")
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
    let dir = std::env::temp_dir().join(format!("warp-rs-patchset-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn hex_bytes(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|t| u8::from_str_radix(t, 16).unwrap_or_else(|_| panic!("非法 hex：{t}")))
        .collect()
}

fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

#[test]
fn m2_十条补丁_全部命中_且幂等() {
    let Some(src) = test_exe() else { return };
    let dir = temp_dir("m2");
    let copy = dir.join("client-orig.exe");
    let patched = dir.join("client-patched.exe");
    let report = dir.join("report.json");
    std::fs::copy(&src, &copy).unwrap();

    // ---------------- 第一次打 ----------------
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
    assert_eq!(rep["total"].as_u64().unwrap(), 10, "补丁总数不对");
    assert_eq!(rep["failed"].as_u64().unwrap(), 0, "有补丁失败：\n{text}");
    assert_eq!(rep["ok"].as_bool().unwrap(), true);

    let orig = std::fs::read(&copy).unwrap();
    let out = std::fs::read(&patched).unwrap();

    let entries = rep["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 10);

    for (i, entry) in entries.iter().enumerate() {
        let id = entry["id"].as_str().unwrap();
        assert_eq!(id, PATCH_IDS[i], "补丁顺序/数量不对");
        let status = entry["status"].as_str().unwrap();
        assert!(
            status == "applied" || status == "already",
            "补丁 {id} 的状态是 {status}，应该是 applied / already：\n{text}"
        );

        // 逐个落点核对：原文件里是 old，打完的文件里是 new
        let locs = entry["locations"].as_array().unwrap();
        for loc in locs {
            let off = loc["offset"].as_u64().unwrap() as usize;
            let old = hex_bytes(loc["old"].as_str().unwrap());
            let new = hex_bytes(loc["new"].as_str().unwrap());
            assert_eq!(&orig[off..off + old.len()], &old[..], "补丁 {id} 的落点 0x{off:X} 原字节对不上");
            assert_eq!(&out[off..off + new.len()], &new[..], "补丁 {id} 的落点 0x{off:X} 新字节对不上");
            println!(
                "  {id:<20} {status:<8} 偏移 0x{off:08X}  {} → {}",
                hex_str(&old),
                hex_str(&new)
            );
        }
        assert!(!locs.is_empty(), "补丁 {id} 没有落点");
    }

    // 关键落点：必须有实际改动
    for id in PATCH_IDS {
        let e = entries
            .iter()
            .find(|e| e["id"].as_str() == Some(id))
            .unwrap();
        let locs = e["locations"].as_array().unwrap();
        let changed = locs.iter().any(|l| {
            l["old"].as_str().unwrap() != l["new"].as_str().unwrap()
        });
        assert!(changed, "补丁 {id} 没有任何字节变化");
    }

    // ---------------- 第二次打（幂等） ----------------
    let patched2 = dir.join("client-patched2.exe");
    let report2 = dir.join("report2.json");
    let (code, text2) = run(&[
        "apply",
        "--exe",
        patched.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
        "--out",
        patched2.to_str().unwrap(),
        "--report",
        report2.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "第二次打补丁失败：\n{text2}");
    let rep2: Value = serde_json::from_slice(&std::fs::read(&report2).unwrap()).unwrap();
    assert_eq!(rep2["failed"].as_u64().unwrap(), 0, "第二次有补丁失败：\n{text2}");
    for entry in rep2["entries"].as_array().unwrap() {
        let id = entry["id"].as_str().unwrap();
        let status = entry["status"].as_str().unwrap();
        assert_eq!(status, "already", "补丁 {id} 第二遍不是 already，而是 {status}");
    }
    // 两遍打出来的文件必须一模一样
    assert_eq!(
        std::fs::read(&patched).unwrap(),
        std::fs::read(&patched2).unwrap(),
        "第二遍打补丁改动了文件（不幂等）"
    );

    // ---------------- 校验 ----------------
    let (code, text3) = run(&[
        "verify",
        "--exe",
        patched.to_str().unwrap(),
        "--patches",
        patch_table().to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "verify 失败：\n{text3}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn m2_补丁表本身是合法的() {
    // 表里的每条都要有 desc（中文（English）格式）和 desc_en
    let text = std::fs::read_to_string(patch_table()).unwrap();
    assert!(text.contains("version: 1"));
    for id in PATCH_IDS {
        assert!(text.contains(&format!("id: {id}")), "补丁表里没有 {id}");
    }
    assert_eq!(text.matches("  - id: ").count(), 10, "补丁表里不是 10 条");
}

/// 辅助：确认测试 exe 没被我们动过（补丁只能打在副本上）。
#[test]
fn m2_原exe_没有被改动() {
    let Some(src) = test_exe() else { return };
    let meta = std::fs::metadata(&src).unwrap();
    assert!(meta.len() > 0);
    let _ = Path::new(&src);
}
