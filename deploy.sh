#!/usr/bin/env bash
# deploy.sh — 编译并发布 opc-cli 到 npm
set -euo pipefail

cd "$(dirname "$0")"

echo "==> 1/2 编译 (npm run build)"
npm run build

if [ ! -f "dist/main.js" ]; then
  echo "ERROR: dist/main.js 不存在，构建失败" >&2
  exit 1
fi

echo "==> 2/2 发布 (npm publish)"
if [ "${1:-}" = "dry-run" ]; then
  npm pack --dry-run
else
  npm publish
fi

echo "✅ 发布完成"
