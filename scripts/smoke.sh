#!/usr/bin/env bash
# 前端冒烟测试的一步运行脚本(CI 与本地通用):用 testdata 启动服务,再用 Playwright 跑 scripts/browser-smoke.js。
#
#   cargo build -p orbitscope && scripts/smoke.sh
#
# 环境变量:
#   ORBITSCOPE_BIN  二进制路径(默认 target/debug/orbitscope)
#   CESIUM_DIR      本地 CesiumJS 的 Build/Cesium 目录(必需;测试不访问 CDN)。获取方法:
#                     npm pack cesium@1.146.0 && tar -xzf cesium-1.146.0.tgz package/Build/Cesium
#   SMOKE_OUT       截图输出目录(默认临时目录)
#   SMOKE_PORT      端口(默认 18080)
# 需要 node 能 require('playwright')(必要时设置 NODE_PATH)。
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${ORBITSCOPE_BIN:-$root/target/debug/orbitscope}"
port="${SMOKE_PORT:-18080}"
: "${CESIUM_DIR:?请设置 CESIUM_DIR 指向 CesiumJS 的 Build/Cesium 目录}"
[ -f "$CESIUM_DIR/Cesium.js" ] || { echo "CESIUM_DIR 中找不到 Cesium.js: $CESIUM_DIR" >&2; exit 1; }
[ -x "$bin" ] || { echo "找不到二进制 $bin(先运行 cargo build -p orbitscope)" >&2; exit 1; }

work="$(mktemp -d)"
out="${SMOKE_OUT:-$work/shots}"
mkdir -p "$out" "$work/data"
server_pid=""
cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  rm -rf "$work/data" "$work/cache" "$work/archive" "$work/home"
}
trap cleanup EXIT

# testdata 的历元在 2024-01-01;把"获取时间"(文件修改时间)设到模拟时刻之前,known-at 策略下才可见
cp "$root"/testdata/spacetrack_gp.json "$root"/testdata/sample.3le "$root"/testdata/celestrak_gp.json "$work/data/"
touch -d "2024-01-01 12:10:00 UTC" "$work"/data/*

cat > "$work/config.toml" <<CONF
[sources]
cache_dir = "$work/cache"
[sources.celestrak]
enabled = false
[sources.files]
paths = ["$work/data/spacetrack_gp.json", "$work/data/sample.3le", "$work/data/celestrak_gp.json"]
[archive]
dir = "$work/archive"
[server]
bind = "127.0.0.1:$port"
cesium_dir = "$CESIUM_DIR"
CONF

# 隔离用户目录与凭据,测试绝不联网采集
env -u ORBITSCOPE_SPACETRACK_USER -u ORBITSCOPE_SPACETRACK_PASSWORD -u ORBITSCOPE_API_TOKEN \
  HOME="$work/home" XDG_CONFIG_HOME="$work/home/.config" XDG_DATA_HOME="$work/home/.data" XDG_CACHE_HOME="$work/home/.cache" \
  "$bin" -c "$work/config.toml" serve --sim-time 2024-01-01T12:30:00Z > "$out/server.log" 2>&1 &
server_pid=$!

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$port/api/v1/status" > /dev/null 2>&1; then break; fi
  kill -0 "$server_pid" 2>/dev/null || { echo "服务启动失败:" >&2; cat "$out/server.log" >&2; exit 1; }
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$port/api/v1/status" > /dev/null || { echo "服务未在 30 秒内就绪" >&2; cat "$out/server.log" >&2; exit 1; }

node "$root/scripts/browser-smoke.js" "http://127.0.0.1:$port" "$out"
echo "截图与服务日志: $out"
