#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""把 WARP 老格式的翻译表（*.txt）转成 warp_rs 的 YAML 补丁表。

WARP 老格式长这样（`-` 开头一条，`desc` / `find` / `replace` / `startDate`）::

    -
      desc : Message Box
      find : <B8 DE BD C3 C1 F6 00>
      replace : "消息"
      startDate : 20100000

新格式（warp_rs 的补丁表）::

    - id: message_box
      desc: Message Box
      find: "B8 DE BD C3 C1 F6 00"
      replace: "CF FB CF A2"
      section: .rdata
      expect: 1
      mode: replace
      dates: { start: 20100000, end: 20991231 }

用法::

    # 只做格式转换（不碰 exe，find/expect 等字段留空）
    python3 scripts/import_warp_txt.py warp_custom_zh.txt -o patches/zh-cn.yml

    # 带客户端 exe：自动算段、命中数、第几次命中、要不要补字符串结束符
    python3 scripts/import_warp_txt.py warp_custom_zh.txt -o patches/zh-cn.yml \
        --exe <fixtures>/2025-06-04_Ragexe_neil.exe

只依赖 Python3 标准库。
"""

import argparse
import os
import re
import struct
import sys

# --------------------------------------------------------------------------
# PE 解析（只取段表，够用就行）
# --------------------------------------------------------------------------


class Section(object):
    def __init__(self, name, va, vsize, raw_ptr, raw_size):
        self.name = name
        self.va = va
        self.vsize = vsize
        self.raw_ptr = raw_ptr
        self.raw_size = raw_size

    @property
    def raw_end(self):
        return self.raw_ptr + self.raw_size

    def contains(self, off):
        return self.raw_ptr <= off < self.raw_end


def read_sections(exe_path):
    """返回 (文件字节, [Section...])。"""
    with open(exe_path, "rb") as f:
        data = f.read()
    if data[:2] != b"MZ":
        raise SystemExit("不是 PE 文件：%s" % exe_path)
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise SystemExit("不是 PE 文件（缺 PE 签名）：%s" % exe_path)
    coff = e_lfanew + 4
    num_sections = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    if magic == 0x20B:
        image_base = struct.unpack_from("<Q", data, opt + 24)[0]
    else:
        image_base = struct.unpack_from("<I", data, opt + 28)[0]
    sec_start = opt + opt_size
    sections = []
    for i in range(num_sections):
        off = sec_start + i * 40
        raw_name = data[off:off + 8]
        name = raw_name.split(b"\0")[0].decode("latin-1")
        vsize, va, raw_size, raw_ptr = struct.unpack_from("<IIII", data, off + 8)
        sections.append(Section(name, va, vsize, raw_ptr, raw_size))
    return data, sections, image_base


# --------------------------------------------------------------------------
# WARP 老格式解析
# --------------------------------------------------------------------------

ENTRY_RE = re.compile(
    r"^\s*-\s*$\s*"
    r"(?:.*?^\s*desc\s*:\s*(?P<desc>.*?)\s*$\s*)?"
    r"^\s*find\s*:\s*<(?P<find>[0-9A-Fa-f\s]*)>\s*$\s*"
    r"^\s*replace\s*:\s*\"(?P<replace>.*)\"\s*$",
    re.M,
)

FIELD_RE = re.compile(r"^\s*(?P<key>[A-Za-z_]+)\s*:\s*(?P<val>.*?)\s*$", re.M)


def parse_warp_txt(text):
    """把老表解析成 [{desc, find(bytes), replace(str), startDate, endDate}]。"""
    entries = []
    # 按 `-` 切成一条条
    chunks = re.split(r"^\s*-\s*$", text, flags=re.M)
    for chunk in chunks:
        if not chunk.strip():
            continue
        m = re.search(r"^\s*find\s*:\s*<([0-9A-Fa-f\s]*)>\s*$", chunk, re.M)
        if not m:
            continue
        find_bytes = bytes(int(x, 16) for x in m.group(1).split())
        if not find_bytes:
            continue
        rm = re.search(r'^\s*replace\s*:\s*"(.*)"\s*$', chunk, re.M)
        replace = rm.group(1) if rm else ""
        dm = re.search(r"^\s*desc\s*:\s*(.*?)\s*$", chunk, re.M)
        desc = dm.group(1) if dm else ""
        dates = {}
        for key in ("startDate", "endDate", "startDateRE", "endDateRE"):
            km = re.search(r"^\s*%s\s*:\s*(\d+)\s*$" % key, chunk, re.M)
            if km:
                dates[key] = int(km.group(1))
        entries.append(
            {
                "desc": desc,
                "find": find_bytes,
                "replace": replace,
                "dates": dates,
            }
        )
    return entries


def slugify(desc, used):
    """desc → 唯一 id。"""
    s = desc.strip().lower()
    s = re.sub(r"[^0-9a-z]+", "_", s).strip("_")
    if not s:
        s = "patch"
    base = s
    i = 2
    while s in used:
        s = "%s_%d" % (base, i)
        i += 1
    used.add(s)
    return s


# --------------------------------------------------------------------------
# 落点选择
# --------------------------------------------------------------------------

# 有些 WARP 条目的 find 太短（2~4 字节），在整个 exe 里能撞上几十上百次。
# 这里按「我们客户端里真正的那个字符串」手工指定落点（文件偏移）。
# 偏移是针对 2025-06-04_Ragexe 客户端的；换了客户端 expect 对不上会直接报错。
TARGETS = {
    # "%d 시간 "（技能计时：时）—— 4 处同构字符串，全打
    "Skill Timer Hour": [0x0C71001, 0x0C71045, 0x0C710A9, 0x0C7114D],
    "Skill Timer Minutes": [0x0C71009, 0x0C7104D, 0x0C710B1, 0x0C71155],
    "Skill Timer Seconds": [0x0C7100F, 0x0C71053, 0x0C710B7, 0x0C7115B],
    # "%d 삭제 %d 분 %d 초 %d 분 %d초 "（删除时间）—— 整串翻译，Part 1 是它的子串
    "Deletion Time - Part 2": [0x0C72A3C],
    "Deletion Time - Part 1": [0x0C72A41],
    # UI 标签
    "Minutes": [0x0C16E29],   # "...Time : %d 분   LogTime : ..."
    "Seconds": [0x0C16E55],   # "...LogTime : %d 초"
    "Map": [0x0C76E31],       # "/맵이동" 里的「맵」
    "Monster": [0x0C76E0D],   # "/몬스터"
    "More": [0x0C7765D],      # "/더"
    "Find": [0x0C28357],      # 独立的「찾기」
    "Back to Nav": [0x0C672E1],
    "Tip box - Part 2": [0x0C11CEC],  # 独立的「닫기」
}

# 这些条目在我们客户端里唯一能匹配上的地方是**文件名**（*.tga），
# 打上去会破坏客户端读图，必须禁用。
DISABLED = {
    "Medium": "唯一命中在文件名 effect\\중간상세화.tga 里，改了会破坏客户端读图",
    "Small": "唯一命中在文件名 effect\\작은상세화.tga 里，改了会破坏客户端读图",
}

# 写入区域重叠时，优先级大的先生效（warp_rs 里 priority 大的先打）
PRIORITY = {
    "Deletion Time - Part 2": 1,   # 整串；Part 1 是它的子串，只能二选一
}


def find_all(data, pat, lo=0, hi=None):
    hits = []
    if hi is None:
        hi = len(data)
    i = data.find(pat, lo, hi)
    while i >= 0:
        hits.append(i)
        i = data.find(pat, i + 1, hi)
    return hits


def section_of(sections, off):
    for s in sections:
        if s.contains(off):
            return s
    return None


def pick_target(data, sections, desc, pat, hits):
    """挑落点：优先手工指定，其次按「独立字符串」打分自动挑。"""
    if desc in TARGETS:
        offs = TARGETS[desc]
        for o in offs:
            if data[o:o + len(pat)] != pat:
                raise SystemExit(
                    "手工指定的落点对不上：%s @ 0x%X（表里的 find 和 exe 不一致）" % (desc, o)
                )
        return list(offs)

    # 自动挑：优先 .rdata/.data、优先「后面紧跟 0x00」的独立字符串
    best = None
    for h in hits:
        sec = section_of(sections, h)
        score = 0
        if sec is not None and sec.name in (".rdata", ".data"):
            score += 4
        if sec is not None and sec.name == ".text":
            score -= 4
        nxt = data[h + len(pat)] if h + len(pat) < len(data) else -1
        pre = data[h - 1] if h > 0 else -1
        if nxt == 0:
            score += 2
        if pre == 0:
            score += 1
        if best is None or score > best[0]:
            best = (score, h)
    if best is None:
        return []
    return [best[1]]


# --------------------------------------------------------------------------
# YAML 输出
# --------------------------------------------------------------------------


def yaml_quote(s):
    return '"%s"' % s.replace("\\", "\\\\").replace('"', '\\"')


def emit(out, entries, table_version=1):
    out.write("# warp_rs 补丁表 —— 由 scripts/import_warp_txt.py 自动生成\n")
    out.write("# 目标客户端：2025-06-04 Ragexe（PE32，未加壳）\n")
    out.write("#\n")
    out.write("# 字段说明：\n")
    out.write("#   id       唯一标识\n")
    out.write("#   desc     说明（启动器界面上显示）\n")
    out.write("#   find     搜索特征串（hex，?? 为通配）\n")
    out.write("#   replace  替换字节（hex）\n")
    out.write("#   section  限定在哪个段里搜\n")
    out.write("#   expect   期望命中次数（不符即失败）\n")
    out.write("#   nth      打第几次命中（1 开始）\n")
    out.write("#   all      是否把所有命中都打上\n")
    out.write("#   terminate 替换后是否补一个 00 字符串结束符\n")
    out.write("#   dates    按 PE 编译时间（YYYYMMDD）过滤\n")
    out.write("#   disabled 是否停用（跳过但不算失败）\n")
    out.write("version: %d\n" % table_version)
    out.write("patches:\n")
    for e in entries:
        out.write("  - id: %s\n" % e["id"])
        out.write("    desc: %s\n" % yaml_quote(e["desc"]))
        out.write("    find: %s\n" % yaml_quote(e["find_hex"]))
        out.write("    replace: %s\n" % yaml_quote(e["replace_hex"]))
        if e.get("section"):
            out.write("    section: %s\n" % e["section"])
        if e.get("expect") is not None:
            out.write("    expect: %d\n" % e["expect"])
        if e.get("nth") is not None:
            out.write("    nth: %d\n" % e["nth"])
        if e.get("all"):
            out.write("    all: true\n")
        if e.get("terminate") is not None:
            out.write("    terminate: %s\n" % ("true" if e["terminate"] else "false"))
        if e.get("priority"):
            out.write("    priority: %d\n" % e["priority"])
        if e.get("disabled"):
            out.write("    disabled: true\n")
            if e.get("note"):
                out.write("    note: %s\n" % yaml_quote(e["note"]))
        out.write("    mode: replace\n")
        if e.get("dates"):
            out.write(
                "    dates: { start: %d, end: %d }\n"
                % (e["dates"]["start"], e["dates"]["end"])
            )
        if e.get("comment"):
            out.write("    # %s\n" % e["comment"])
    return out


def main():
    ap = argparse.ArgumentParser(description="WARP 老翻译表 → warp_rs YAML 补丁表")
    ap.add_argument("input", help="WARP 老格式的 .txt")
    ap.add_argument("-o", "--output", required=True, help="输出 YAML")
    ap.add_argument(
        "--exe",
        default=os.environ.get("WARP_RS_TEST_EXE"),
        help="客户端 exe（给了才能算出 section / expect / nth / terminate）",
    )
    ap.add_argument(
        "--start", type=int, default=20100000, help="起始日期 YYYYMMDD（默认 20100000）"
    )
    ap.add_argument(
        "--end", type=int, default=20991231, help="结束日期 YYYYMMDD（默认 20991231）"
    )
    args = ap.parse_args()

    with open(args.input, encoding="utf-8") as f:
        text = f.read()
    raw = parse_warp_txt(text)
    if not raw:
        raise SystemExit("没解析出任何条目：%s" % args.input)

    data = sections = None
    if args.exe:
        if not os.path.exists(args.exe):
            raise SystemExit("exe 不存在：%s" % args.exe)
        data, sections, _base = read_sections(args.exe)

    used = set()
    out_entries = []
    for item in raw:
        desc = item["desc"]
        pat = item["find"]
        try:
            repl = item["replace"].encode("gbk")
        except UnicodeEncodeError as e:
            raise SystemExit("替换文本无法用 GBK 编码：%r（%s）" % (item["replace"], e))
        if len(repl) > len(pat):
            # 原地覆写放不下 → 跳过并报警
            print(
                "警告：%s 的替换（%d 字节）比 find（%d 字节）长，跳过"
                % (desc, len(repl), len(pat)),
                file=sys.stderr,
            )
            continue

        e = {
            "id": slugify(desc, used),
            "desc": desc,
            "find_hex": " ".join("%02X" % b for b in pat),
            "replace_hex": " ".join("%02X" % b for b in repl),
            "dates": {
                "start": item["dates"].get("startDate", args.start),
                "end": item["dates"].get("endDate", args.end),
            },
            "expect": None,
            "nth": None,
            "all": False,
            "terminate": None,
            "section": None,
            "disabled": False,
            "note": None,
            "priority": PRIORITY.get(desc, 0),
        }

        if desc in DISABLED:
            e["disabled"] = True
            e["note"] = DISABLED[desc]
            out_entries.append(e)
            continue

        if data is not None:
            hits = find_all(data, pat)
            if not hits:
                print("警告：%s 在 exe 里找不到，跳过" % desc, file=sys.stderr)
                continue
            targets = pick_target(data, sections, desc, pat, hits)
            if not targets:
                print("警告：%s 找不到落点，跳过" % desc, file=sys.stderr)
                continue
            sec = section_of(sections, targets[0])
            e["section"] = sec.name if sec else None
            if sec:
                in_sec = find_all(data, pat, sec.raw_ptr, sec.raw_end)
            else:
                in_sec = hits
            e["expect"] = len(in_sec)
            if len(targets) > 1:
                e["all"] = True
            else:
                e["nth"] = in_sec.index(targets[0]) + 1
            # 后面紧跟 0x00 → 这是一个完整字符串，补结束符最安全
            t = targets[0]
            nxt = data[t + len(pat)] if t + len(pat) < len(data) else -1
            e["terminate"] = (nxt == 0)
            if len(targets) > 1:
                e["comment"] = "同一字符串在 exe 里出现 %d 次，全部替换" % len(targets)
            e["_targets"] = targets

        out_entries.append(e)

    # 写入区域重叠的两条补丁只能留一条（原地覆写会互相踩）
    # 优先级大的先生效，另一条停用并写明原因
    order = sorted(
        range(len(out_entries)),
        key=lambda i: (-out_entries[i].get("priority", 0), i),
    )
    claimed = []  # [(起, 止, 条目 id)]
    for i in order:
        e = out_entries[i]
        targets = e.get("_targets") or []
        if not targets or e.get("disabled"):
            continue
        plen = len(e["replace_hex"].split())
        wlen = plen + (1 if e.get("terminate") else 0)
        flen = len(e["find_hex"].split())
        regions = [(t, t + wlen) for t in targets]
        hit = None
        for (s0, s1) in regions:
            for (c0, c1, cid) in claimed:
                if s0 < c1 and c0 < s1:
                    hit = cid
                    break
            if hit:
                break
        if hit:
            e["disabled"] = True
            e["note"] = "写入区域和补丁 %s 重叠（同一段字符串的两种翻译），只能二选一" % hit
            e["note"] += "；本条保留原韩文"
            e["_targets"] = []
        else:
            for (s0, s1) in regions:
                claimed.append((s0, s1, e["id"]))

    for e in out_entries:
        e.pop("_targets", None)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as f:
        emit(f, out_entries)
    print("写出 %d 条 → %s" % (len(out_entries), args.output))


if __name__ == "__main__":
    main()
