#!/usr/bin/env bash
# 由「已验证可启动的 WARP 产物」生成逐字节等价补丁表
#
# 用法：tools/warp_rs/scripts/gen-parity-table.sh <pristine.exe> <warp_out.exe> <输出.yml>
#
# 原理：把两份 exe 的每个差异块写成一条 mode: offset 补丁
#       （offset + verify 原字节 + replace 新字节），
#       这样我们的引擎产出的 exe 与 WARP 输出逐字节相同。
set -euo pipefail
PRISTINE="${1:?用法: gen-parity-table.sh <pristine.exe> <warp_out.exe> <out.yml>}"
WARP="${2:?}"
OUT="${3:?}"

python3 - "$PRISTINE" "$WARP" "$OUT" <<'PY'
import sys
p, w, out = sys.argv[1], sys.argv[2], sys.argv[3]
P = open(p,'rb').read(); W = open(w,'rb').read()
assert len(P) == len(W), f"文件长度不同：{len(P)} vs {len(W)}"
blocks=[]; i=0; n=len(P)
while i < n:
    if P[i] != W[i]:
        j=i
        while j < n and P[j] != W[j]: j+=1
        blocks.append((i,j)); i=j
    else:
        i+=1
lines = ["# 由 gen-parity-table.sh 自动生成：WARP 产物的逐字节等价补丁表",
         "meta:", "  encoding: gbk", "version: 1", "patches:"]
for k,(a,b) in enumerate(blocks,1):
    lines.append(f'  - id: parity-{k:04d}')
    lines.append(f'    desc: "WARP 等价补丁 {k}（偏移 0x{a:08X}）"')
    lines.append(f'    desc_en: "WARP parity patch {k} @0x{a:08X}"')
    lines.append('    mode: offset')
    lines.append(f'    offset: "0x{a:08X}"')
    lines.append('    verify: "' + ' '.join(f'{x:02X}' for x in P[a:b]) + '"')
    lines.append('    replace: "' + ' '.join(f'{x:02X}' for x in W[a:b]) + '"')
open(out,'w',encoding='utf-8').write('\n'.join(lines)+'\n')
print(f"差异块 {len(blocks)} 个 / {sum(b-a for a,b in blocks)} 字节 → {out}")
PY
