#!/usr/bin/env bash
# ============================================================================
# check-no-private-strings.sh —— 开源仓库「去痕迹」自检
#
# 为什么需要：本仓库是从私有项目里抽出来的工具实现。私有项目的名字、品牌词、
# 内网地址一旦写进源码/文档/提交信息，就会永久留在公开历史里。
# 本脚本在 CI 与本地提交前跑一遍，命中即失败。
#
# 注意：关键词本身也不能以明文出现在本文件里（否则自检自己就是污染源），
#      所以统一用 printf 转义拼接，源码里看不到原词。
#
# 用法:
#   scripts/check-no-private-strings.sh            # 检查工作区
#   scripts/check-no-private-strings.sh --history  # 额外检查提交信息
# ============================================================================
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

# 需要屏蔽的词（大小写不敏感）。新增时保持「短、明确、不易误伤」。
PATTERNS=(
  "$(printf 'yi\x67u')"                       # 私有项目品牌（拉丁）
  "$(printf '\xe4\xb8\x80\xe5\x8f\xa4')"       # 私有项目品牌（中文写法一）
  "$(printf '\xe4\xba\xbf\xe5\x8f\xa4')"       # 私有项目品牌（中文写法二）
  "$(printf 'acen\x65il')"                     # 私有账号（平台一）
  "$(printf 'acen\x69u')"                      # 私有账号（平台二）
  "$(printf '\xe5\x86\xb0\xe5\x9f\x8e')"       # 内部代号
  "$(printf '192\.168\.123')"                  # 内网地址段
)

rc=0

echo "== 检查工作区文件 =="
for pat in "${PATTERNS[@]}"; do
  hits=$(grep -rin --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules \
    -e "$pat" . || true)
  if [ -n "$hits" ]; then
    echo "✗ 命中关键词（长度 ${#pat}）："
    echo "$hits" | sed 's/^/    /'
    rc=1
  fi
done

if [ "${1:-}" = "--history" ]; then
  echo "== 检查提交信息 =="
  for pat in "${PATTERNS[@]}"; do
    hits=$(git log --all --oneline | grep -i -e "$pat" || true)
    if [ -n "$hits" ]; then
      echo "✗ 提交信息命中关键词（长度 ${#pat}）："
      echo "$hits" | sed 's/^/    /'
      rc=1
    fi
  done
fi

if [ "$rc" -eq 0 ]; then
  echo "✓ 干净，没有私有项目痕迹"
fi
exit "$rc"
