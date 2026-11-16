# OrbitScope

准实时近地轨道空间物体监视程序(Rust)。从公开数据源获取轨道根数,本地 SGP4 外推,
按高度范围(默认 100–1000 km)与最小尺寸(默认 ≥ 10 cm)过滤,在浏览器 3D 地球、命令行和 API 中展示。

- **数据源**:CelesTrak(免账号)、Space-Track(免费账号,完整编目含全部碎片)、本地 TLE / OMM 文件;
  磁盘缓存,严格遵守数据源的频率规定,支持离线运行
- **计算**:SGP4/SDP4 外推,3 万个物体全量外推约 8 ms(4 核)
- **过滤**:高度(轨道区间或瞬时高度)、尺寸(基于 RCS 估算)、类型、国家、倾角、名称/编号搜索
- **历史归档**:每组根数只写一次并记录获取时间,永不覆盖;另存有容量上限的原始快照;
  指定任意时刻按"当时已获取的数据"复现展示;三种采集方式(Web 服务 / 常驻进程 / 系统定时任务)
- **展示**:CesiumJS 3D 地球(实时点云、轨道线、统计、时间回放)、命令行(表格 / JSON / CSV)、REST + WebSocket API

## 快速开始

```bash
cargo build --release                             # 需要 Rust 1.88+
./target/release/orbitscope serve                 # 浏览器打开 http://127.0.0.1:8080/
```

或使用 Docker(镜像内置 CesiumJS,浏览器无需访问 CDN):

```bash
docker compose up -d
```

命令行示例:

```bash
orbitscope fetch                                  # 获取并缓存数据
orbitscope list --alt 400..450 --type debris --sort alt
orbitscope show 25544 --track 90m --step 5m
orbitscope export -f csv -o leo.csv
orbitscope stats --bin 100
orbitscope history status                         # 历史归档占用
orbitscope --policy known-at list --at 2026-11-16T08:00:00Z   # 复现某时刻的数据
```

启用 Space-Track:配置 `[sources.spacetrack] enabled = true`,并设置环境变量
`ORBITSCOPE_SPACETRACK_USER` / `ORBITSCOPE_SPACETRACK_PASSWORD`。

> **关于尺寸**:公开编目不包含物体物理尺寸,"尺寸"为雷达截面积(RCS)推算的等效直径估算值。
> 美国空间监视网在近地轨道的编目门限约 10 cm,公开编目基本即为 ≥ 10 cm 的物体。

## 文档

| 文档 | 内容 |
|------|------|
| [使用手册](docs/05-user-guide.md) | 安装、配置、命令行、Web 界面、API、部署、常见问题 |
| [项目规划](docs/01-project-plan.md) | 范围、里程碑、风险 |
| [需求分析](docs/02-requirements.md) | 功能与非功能需求 |
| [设计文档](docs/03-design.md) | 架构、数据模型、算法、API 与 WebSocket 协议、架构决策 |
| [调研](docs/04-references.md) | 公开数据服务与开源项目 |
| [更新日志](CHANGELOG.md) | 版本变化 |

## 项目结构

```
crates/orbit-core     核心计算(解析、外推、坐标、尺寸、过滤),无 IO
crates/orbit-sources  数据源、缓存、限速
crates/orbit-archive  历史归档(SQLite 根数历史库、完整快照、时刻策略)
crates/orbit-server   REST + WebSocket 服务,内嵌前端
crates/orbitscope     命令行程序
web/                  浏览器前端(原生 ES Module)
```

## 许可证

MIT 或 Apache-2.0,任选其一。数据版权归各数据源所有;使用 Space-Track 数据须遵守其用户协议。
