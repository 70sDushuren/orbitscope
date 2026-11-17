# CLAUDE.md

本文件为 Claude Code 在本仓库工作时的指引。

## 项目简介

OrbitScope:用 Rust 编写的准实时近地轨道空间物体监视程序。从公开数据源(Space-Track、CelesTrak、本地文件)获取 GP 轨道根数,
本地 SGP4 外推,按可配置的高度范围(默认 100–1000 km)和最小尺寸(默认 10 cm,基于 RCS 估算)过滤,通过 Web 3D 地球 / CLI / API 展示。

**动手前先读**:`docs/01-project-plan.md`(范围与里程碑)、`docs/02-requirements.md`(需求编号 FR-*/NFR-*)、
`docs/03-design.md`(架构、数据模型、API、ADR)、`docs/04-references.md`(数据源与参考项目)。
设计变更须同步更新对应文档;新的架构决策追加到 `03-design.md` 的 ADR 表。

## 仓库结构

```
crates/orbit-core     纯计算库:模型、OMM/TLE 解析、SGP4 外推、坐标转换、尺寸估算、过滤。禁止 IO / async / 网络
crates/orbit-sources  数据源 trait 与实现、磁盘缓存、限速、SATCAT 合并(async)
crates/orbit-archive  历史归档:SQLite 根数历史库(只追加)、完整快照(有上限)、时刻策略、采集策略类型
crates/orbit-server   axum REST + WebSocket、调度器、Ticker、各连接的"指定时刻"视图
crates/orbitscope     二进制 CLI(clap):fetch / list / show / export / stats / config / collect / history / serve
web/                  CesiumJS 前端(原生 ES Module,无构建步骤),rust-embed 打包进 orbit-server
scripts/              smoke.sh + browser-smoke.js(Playwright 冒烟测试)、gen_synthetic.py(合成数据)、i18n/(多语言检查与自动翻译)
.github/              CI(含浏览器冒烟测试)、Release(二进制 + Docker 镜像)、Prepare release、Security audit、dependabot.yml
testdata/             离线测试样本
config/               示例配置
```
依赖方向只能是 `orbitscope → orbit-server → orbit-archive → orbit-sources → orbit-core`。

## 常用命令

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p orbitscope -- --help
cargo run -p orbitscope -- -c <配置> serve --sim-time 2024-01-01T12:30:00Z   # 用 testdata 演示服务
cargo run -p orbitscope -- -c <配置> --policy nearest-epoch list --at 2024-01-01T12:30:00Z --any-age   # testdata 历元在 2024 年,known-at 下需文件修改时间早于 --at
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
- 服务端集成测试在 127.0.0.1:0 上启动真实服务,用 `time_offset` 把时钟拨到 testdata 的历元附近。
- WS 帧格式(`orbit-server/src/frame.rs`)变更时必须升级版本号并同步 `docs/03-design.md` 5.3 节与前端解码。
- 前端:不引入 npm 构建工具;改 JS 后运行 `node --check web/js/*.js`;界面文字多语言(见下,源语言为简体中文);
  颜色只用 `app.css` 中的角色令牌,类型颜色改动须重新跑 dataviz 调色板校验(`--pairs all`,深色)。
- 前端改动后运行 `scripts/smoke.sh` 验证(CI 的 frontend 任务也跑它):`cargo build -p orbitscope` 后设置
  `CESIUM_DIR`(`npm pack cesium@1.146.0 && tar -xzf cesium-1.146.0.tgz package/Build/Cesium`)与能找到 playwright 的 `NODE_PATH`。
  脚本自动复制 testdata、把文件时间设到模拟时刻之前、启动服务(`--sim-time 2024-01-01T12:30:00Z`)再跑 `browser-smoke.js`。
- 升级 CesiumJS 版本时同步五处:`state.rs` 的 `DEFAULT_CESIUM_URL` 与两个 SRI 常量、`Dockerfile` 与 `ci.yml` 的版本号和 SHA-256、
  示例配置与文档中的版本号。
- `rust-version = "1.89"`(let-chains 需要 1.88;rusqlite 0.43 需要 1.89);提高 MSRV 时同步 CI 的 msrv 任务、README 与使用手册。
- 依赖安全:`cargo audit` 必须通过(Security audit 工作流每周运行);Dependabot 每周开依赖更新 PR。
- 服务需同时响应 SIGINT 与 SIGTERM(容器 / systemd 停止服务依赖 SIGTERM)。
- 历史库只追加:同一 (norad, epoch, source) 先写入者为准,`first_seen` 取数据源获取时间(`GpRecord.fetched_at`),
  绝不改写已有行;清理时每个物体最新一条永不删除。改动 `history.rs` 的选择 SQL 须保持 known-at 的可复现性(有测试)。
- 测试与 CLI 端到端测试必须把 `[archive] dir` 指向临时目录(CLI 测试还隔离了 HOME / XDG_* 环境变量)。
- 服务端设置(时刻策略、采集策略)可运行期修改,对所有访问者生效;非 serve 采集模式下服务端绝不联网。
- "指定时刻"不是服务端设置:它按 WS 连接生效(`state.rs` 的 `View`、`view.rs`、`ws.rs`),REST 用 `view=<编号>` 跟随;
  服务端时钟(`AppState::now`)只受 `--sim-time` 影响。新增依赖时刻的 REST 接口时用 `scope_of()` 取编目与时钟,不要直接读 `state.catalog`。
- 写操作鉴权(`api.rs` 的 `check_write`):有 token 时 Bearer;无 token 时只接受本机直接发起的请求,带反向代理转发头的一律拒绝。
- 对外的错误必须带稳定的 `code`(`errors.rs` 的 `Coded` / `ApiError::coded`),并在 `web/i18n/zh-CN.json` 加 `err.<code>` 词条;
  新增代码时同步 `docs/03-design.md` 5.2 节的代码列表。数据源错误的代码见 `SourceError::code`(词条 `err.source_<code>`)。
- 界面文字一律写进 `web/i18n/zh-CN.json`(源语言)并用 `t()` / `data-i18n*` 引用,不在 JS/HTML 中硬编码可见文字;
  新增或修改源文字后,运行 `node scripts/i18n/i18n.mjs translate`(需 ANTHROPIC_API_KEY)或人工补译 `en.json`、`zh-HK.json`
  后运行 `stamp`;`check` 必须通过才能发布。`zh-HK` 用香港粵語書面語。
- 发布走 GitHub Actions 的 "Prepare release"(自动补译、改版本号、打标签、调用 Release):先更新 `CHANGELOG.md`
  (把"未发布"改为版本号;Release 页面的说明取自 CHANGELOG 中该版本的一节)。Release 构建四个平台的二进制(Linux x86_64 / arm64、macOS arm64、Windows x86_64)并把
  多架构 Docker 镜像推到 ghcr.io。已有标签发布失败时用 Release 工作流的 "Run workflow" 输入标签重跑,不要移动标签。
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
- [x] M2 orbit-sources(26 个测试,全部基于 wiremock;CelesTrak 真实接口已在用户本地验证:19,575 个物体)
- [x] M3 CLI(`fetch/list/show/export/stats/config`,TOML 配置;10 个端到端测试)
- [x] M4 orbit-server(REST + WebSocket OSF1 帧、随机分钟调度、token/本机写鉴权)
- [x] M5 Web 前端(CesiumJS 3D 地球、过滤/搜索/统计/详情;浏览器冒烟测试 `scripts/browser-smoke.js`)
- [x] M6 打包发布(Dockerfile 内置 Cesium、compose、systemd、CI / Release 工作流、使用手册、许可证)
- [x] v0.2.1 界面多语言(简体 / English / 繁體香港)与发布时自动翻译
- [x] v0.2 历史归档与采集策略(orbit-archive;known-at / nearest-epoch / latest;serve / daemon / external;Web 设置面板)
- [x] v0.2.1 已发布(GitHub Release,三平台二进制)
- [x] v0.3.0:反向代理下的写鉴权修复、网页 token 输入、"指定时刻"按连接生效、错误信息多语言、Cesium SRI、
  Linux arm64 与 Docker 镜像发布、浏览器冒烟测试进 CI、依赖安全检查(MSRV 1.89)
