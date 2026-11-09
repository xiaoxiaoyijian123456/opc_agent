#!/usr/bin/env bash
# deploy.sh — 可选的本地开发部署脚本；普通安装使用 cargo install --path .
set -euo pipefail

cd "$(dirname "$0")"

echo "==> 1/3 编译 (cargo build --release)"
cargo build --release

MYOPC_DIR="$HOME/.myopc"
BIN_DIR="$MYOPC_DIR/bin"
mkdir -p "$BIN_DIR"
cp target/release/myopc "$BIN_DIR/myopc"
chmod +x "$BIN_DIR/myopc"

echo "==> 2/3 同步 scaffold"
mkdir -p "$MYOPC_DIR"
rm -rf "$MYOPC_DIR/scaffold"
cp -R scaffold "$MYOPC_DIR/scaffold"

echo "==> 3/3 默认配置 myopc.json"
if [ ! -f "$MYOPC_DIR/myopc.json" ]; then
  cp scaffold/myopc.json.example "$MYOPC_DIR/myopc.json"
  echo "  已生成默认配置 $MYOPC_DIR/myopc.json"
else
  echo "  已存在，跳过（不覆盖）$MYOPC_DIR/myopc.json"
fi

echo "✅ 完成。运行: myopc init / myopc"
