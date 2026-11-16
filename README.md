# OrbitScope

准实时近地轨道空间物体监视程序(Rust)。

- 数据源:Space-Track(需账号,全量编目)/ CelesTrak(免账号)/ 本地 TLE、OMM 文件
- SGP4 本地外推,位置每秒刷新
- 过滤:高度范围(默认 100–1000 km)、最小尺寸(默认 10 cm,基于 RCS 估算)、类型、国家、搜索
- 展示:Web 3D 地球(CesiumJS)、列表、详情与轨迹、统计;CLI 与 REST/WebSocket API

> 状态:M1–M5 已完成(核心库、数据源、命令行、Web 服务、3D 前端);M6 打包发布进行中。见 [docs/](docs/)。

## 快速开始

```bash
cargo build --release
./target/release/orbitscope config init          # 生成 ~/.config/orbitscope/config.toml(可选)
./target/release/orbitscope fetch                # 下载并缓存数据(遵守数据源刷新间隔)
./target/release/orbitscope list                 # 默认:100–1000 km、≥10 cm
./target/release/orbitscope list --alt 400..450 --type debris --sort alt
./target/release/orbitscope list --alt-mode instantaneous --at +30m
./target/release/orbitscope show 25544 --track 90m --step 5m
./target/release/orbitscope export -f csv -o leo.csv
./target/release/orbitscope stats --bin 100
./target/release/orbitscope --offline list        # 只用缓存
./target/release/orbitscope serve                 # 浏览器打开 http://127.0.0.1:8080/ 查看 3D 地球
```

启用 Space-Track(完整编目,含全部碎片):在配置中设 `[sources.spacetrack] enabled = true`,并设置环境变量
`ORBITSCOPE_SPACETRACK_USER` / `ORBITSCOPE_SPACETRACK_PASSWORD`。

浏览器端的 3D 引擎 CesiumJS 默认从 jsDelivr CDN 加载。完全离线使用时:

```bash
npm pack cesium@1.146.0 && tar -xzf cesium-1.146.0.tgz      # 得到 package/Build/Cesium
# 在配置 [server] 中设置 cesium_dir = "/绝对路径/package/Build/Cesium"
```

## 文档

| 文档 | 内容 |
|------|------|
| [01-project-plan.md](docs/01-project-plan.md) | 愿景、范围、里程碑、风险 |
| [02-requirements.md](docs/02-requirements.md) | 功能与非功能需求 |
| [03-design.md](docs/03-design.md) | 架构、数据模型、算法、API、ADR |
| [04-references.md](docs/04-references.md) | 开源软件与公开数据服务调研 |

**关于尺寸**:公开编目不包含物体物理尺寸,本程序的尺寸为雷达截面积(RCS)推算的等效直径估算值。
美国空间监视网在 LEO 的编目门限约 10 cm,公开编目基本即为 ≥10 cm 物体。
