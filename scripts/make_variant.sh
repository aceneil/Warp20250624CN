#!/usr/bin/env bash
# make_variant.sh —— 从一份未打补丁的客户端，生成指定语言/配置的可执行文件
#
# 设计：客户端本体永远不动。每次切换语言，都从 pristine 副本重新打一遍补丁，
# 得到确定的结果（不叠加、不漂移），所以 en ⇄ cn 可以随时来回切。
#
# 用法：
#   make_variant.sh <pristine.exe> <out.exe> <lang|表路径|none> [额外补丁表...]
#
#   lang 可以是：
#     none          —— 不打任何语言包
#     zh-cn / en    —— 语言代码，对应 patches/lang/<代码>.yml
#     任意表路径     —— 例如 patches/lang/ja.yml（自制语言包直接给路径）
#
# 例：
#   ./scripts/make_variant.sh Ragexe.pristine.exe Ragexe.exe zh-cn patches/base.yml
#   ./scripts/make_variant.sh Ragexe.pristine.exe Ragexe.exe en    patches/base.yml
#   ./scripts/make_variant.sh Ragexe.pristine.exe Ragexe.exe patches/lang/ja.yml
#
# 退出码：0 成功；非 0 表示某一步失败（此时不会覆盖 out.exe）
set -euo pipefail

PRISTINE="${1:?用法: make_variant.sh <pristine.exe> <out.exe> <lang|表路径|none> [补丁表...]}"
OUT="${2:?缺少输出路径}"
LANG_SEL="${3:?缺少语言（语言代码 / 表路径 / none）}"
shift 3 || true

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
BIN="${WARP_RS_BIN:-$ROOT/target/release/warp_rs}"
[ -x "$BIN" ] || { echo "[variant] 找不到引擎：$BIN（先 cargo build --release）" >&2; exit 1; }
[ -f "$PRISTINE" ] || { echo "[variant] 找不到 pristine：$PRISTINE" >&2; exit 1; }

# 语言 → 补丁表：先当路径找，再当语言代码找 patches/lang/<代码>.yml
resolve_table() {
  local sel="$1"
  if [ -f "$sel" ]; then printf '%s\n' "$sel"; return 0; fi
  local cand="$ROOT/patches/lang/$sel.yml"
  [ -f "$cand" ] && { printf '%s\n' "$cand"; return 0; }
  cand="$ROOT/patches/lang/$sel.yaml"
  [ -f "$cand" ] && { printf '%s\n' "$cand"; return 0; }
  return 1
}

TABLES=()
case "$LANG_SEL" in
  none) : ;;
  *)
    if TABLE="$(resolve_table "$LANG_SEL")"; then
      TABLES+=("$TABLE")
    else
      echo "[variant] 找不到语言包：$LANG_SEL（语言代码要在 patches/lang/ 下有同名 .yml，或直接给表路径）" >&2
      exit 1
    fi
    ;;
esac
for extra in "$@"; do TABLES+=("$extra"); done

# 逐张表叠加；任何一张失败就整体失败
CUR="$PRISTINE"
TMPDIR_="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_"' EXIT
i=0
for t in "${TABLES[@]}"; do
  [ -f "$t" ] || { echo "[variant] 补丁表不存在：$t" >&2; exit 1; }
  i=$((i+1))
  NEXT="$TMPDIR_/stage$i.exe"
  echo "[variant] 应用 $(basename "$t")"
  "$BIN" apply --exe "$CUR" --patches "$t" --out "$NEXT" --report "$TMPDIR_/stage$i.json" >/dev/null
  CUR="$NEXT"
done

cp -f "$CUR" "$OUT"
echo "[variant] 已生成 $OUT（语言=$LANG_SEL，补丁表 ${#TABLES[@]} 张）"
