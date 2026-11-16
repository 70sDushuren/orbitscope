# CLAUDE.md

本文件为 Claude Code 在本仓库工作时的指引。

## 项目简介

OrbitScope:用 Rust 编写的准实时近地轨道空间物体监视程序。从公开数据源(Space-Track、CelesTrak、本地文件)获取 GP 轨道根数,
本地 SGP4 外推,按可配置的高度范围(默认 100–1000 km)和最小尺寸(默认 10 cm,基于 RCS 估算)过滤,通过 Web 3D 地球 / CLI / API 展示。

**动手前先读**:`docs/01-project-plan.md`(范围与里程碑)、`docs/02-requirements.md`(需求编号 FR-*/NFR-*)、
`docs/03-design.md`(架构、数据模型、API、ADR)、`docs/04-references.md`(数据源与参考项目)。
设计变更须同步更新对应文档;新的架构决策追加到 `03-design.md` 的 ADR 表。

## 仓库结构(目标)

```
crates/orbit-core     纯计算库:模型、OMM/TLE 解析、SGP4 外推、坐标转换、尺寸估算、过滤。禁止 IO / async / 网络
crates/orbit-sources  数据源 trait 与实现、磁盘缓存、限速、SATCAT 合并(async)
crates/orbit-server   axum REST + WebSocket、调度器、Ticker
crates/orbitscope     二进制 CLI(clap):fetch / list / show / export / stats / serve
web/                  CesiumJS 前端,rust-embed 打包
testdata/             离线测试样本
config/               示例配置
```
依赖方向只能是 `orbitscope → orbit-server → orbit-sources → orbit-core`。

## 常用命令

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p orbitscope -- --help
cargo run -p orbitscope -- -c <配置> list --at 2024-01-01T12:30:00Z --any-age   # 配合 testdata 的 files 源
cargo bench -p orbit-core
cargo run -p orbit-core --example demo -- testdata/spacetrack_gp.json 2024-01-01T12:30:00Z
cargo run -p orbit-sources --example fetch -- --offline        # 只用缓存
cargo run -p orbit-sources --example fetch -- --spacetrack     # 需设置凭据环境变量
```

## 编码约定

- Rust stable,2024 edition;提交前 `cargo fmt` + `clippy -D warnings` + `cargo test` 必须通过。
- 库 crate 用 `thiserror`,二进制用 `anyhow`;库代码不得 `unwrap()`/`expect()`(测试除外)。
- 单位写进命名:`_km`、`_kms`、`_m`、`_deg`、`_rad`、`_min`;时间统一 `chrono::DateTime<Utc>`。
- NORAD 编号用 `u32`(必须支持 9 位编号);内部主格式为 OMM,不要依赖 TLE 列宽。
- 坐标系:SGP4 输出 TEME → GMST 旋转得 ECEF → WGS84 大地坐标。函数名标明坐标系,如 `teme_to_ecef`。
- 单个物体解析/外推失败只计数并记录日志,绝不中断整批处理。
- 尺寸始终是"估算",对外数据必须带 `size_source` 字段。
- 测试中允许 `unwrap()`(见 `clippy.toml`);引用文献参考值的测试模块可 `allow(clippy::inconsistent_digit_grouping)`。
- SGP4 使用 sgp4 crate 的默认(改进)模式,而非 AFSPC 兼容模式(差异数十米,见 ADR-009)。
- `config/orbitscope.example.toml` 必须与代码默认值一致(有单元测试校验);新增配置项时同步更新。
- CLI 端到端测试只使用 `[sources.files]` 指向 testdata,并禁用 CelesTrak。
- 公共 API 写 rustdoc;注释和文档可用中文,标识符用英文。

## 数据源规则(必须遵守)

- **测试和 CI 绝不访问真实 Space-Track / CelesTrak**,使用 `testdata/` 与 `wiremock`。
- 开发中手动调试真实接口时也要走缓存,避免重复下载;Space-Track GP 全量 ≤ 1 次/小时,全局请求 ≤ 20 次/分。
- 凭据只从环境变量 `ORBITSCOPE_SPACETRACK_USER` / `ORBITSCOPE_SPACETRACK_PASSWORD` 或不入库的本地配置读取;
  不得写入代码、配置示例、日志、测试或提交记录。`.gitignore` 已忽略 `*.local.toml` 与 `.env`。
- HTTP `User-Agent` 统一为 `orbitscope/<version>`,不附带任何个人信息。
- 不得把 Space-Track 原始数据提交到仓库(用户协议限制再分发);`testdata/` 只放少量公开样本或合成数据。

## 开发环境说明

- 云端开发环境访问 celestrak.org 返回 500 / 连接重置(疑似拒绝数据中心 IP),**不要反复重试**(CelesTrak 会封禁产生大量错误的 IP);
  需要真实 CelesTrak 数据时请在本地运行。www.space-track.org 可访问。

## 当前进度

- [x] M0 规划文档
- [x] M1 orbit-core(44 个测试;基准:3 万物体单线程 ~30 ms,4 核并行 ~8 ms)
- [x] M2 orbit-sources(26 个测试,全部基于 wiremock;CelesTrak 真实接口尚未在本地验证)
- [x] M3 CLI(`fetch/list/show/export/stats/config`,TOML 配置;10 个端到端测试)
- [ ] M4 orbit-server
- [ ] M5 Web 前端
- [ ] M6 打包发布
