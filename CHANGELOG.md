# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),版本号遵循语义化版本。

## [0.3.0] - 2026-11-17

### 安全
- 未设置 API token 时,写接口不再接受经反向代理转发的请求(带有 `X-Forwarded-For`、`X-Real-IP`、`Forwarded`、
  `X-Forwarded-Proto`、`X-Forwarded-Host` 或 `Via` 请求头)。此前同机反向代理转发的请求来源是本机地址,
  会被当作本机请求放行。**经反向代理对外提供服务时必须设置 `ORBITSCOPE_API_TOKEN`。**
- 从默认 CDN 加载 CesiumJS 时,入口脚本与样式表带子资源完整性校验(SRI),内容被替换时浏览器拒绝加载。
- 升级 `rusqlite` 0.40 → 0.43(内置 SQLite 的 RUSTSEC-2026-0459)与 `time` 0.3.45 → 0.3.55(RUSTSEC-2026-0009)。

### 变更
- **"指定时刻"改为按连接生效**:Web 界面中切换到某个时刻只影响当前页面,其他访问者仍看到实时内容;
  切换时间不再是写操作,不需要 API token。
  - WebSocket 订阅消息增加 `time`(RFC3339 或 `"live"`);catalog 消息增加 `view`、`time_mode`、`time`。
  - REST 的 `/status`、`/objects`、`/objects/{norad}`、`/objects/{norad}/track`、`/stats` 接受 `view=<编号>`,
    与对应连接使用同一时钟与数据。
  - **不兼容**:`PUT /api/v1/settings` 不再接受 `time`;`GET /api/v1/settings` 不再返回 `time_mode`、`server_time`、
    `time_offset_secs`(`/status` 中仍有 `server_time`、`time_offset_secs`,并新增 `time_mode`、`view`)。
- 最低 Rust 版本提高到 1.89(修复上述 SQLite 公告的 `rusqlite` 版本需要)。
- 轨迹接口(`/objects/{norad}/track`)的外推放到阻塞线程池,不再占用异步工作线程。

### 新增
- Web 界面"时间与数据"面板可填写 API token(只保存在本浏览器中),用于 Docker、反向代理或其他电脑访问时修改设置;
  服务端要求 token 时自动显示输入框。
- 错误信息多语言:API 与 WebSocket 的错误增加稳定的 `code` 与 `params`,界面按所选语言显示;
  `/status` 中失败的数据源增加 `error_code`、`error_detail`。`GET /api/v1/settings` 增加 `token_required`。
- 英文说明 `README.en.md`。
- 发布:增加 Linux arm64 二进制;多架构 Docker 镜像(amd64 / arm64)发布到 `ghcr.io/70sdushuren/orbitscope`。
- CI:在真实浏览器中运行前端冒烟测试(`scripts/smoke.sh`,Playwright + 本地 CesiumJS);
  依赖安全检查(每周及依赖变化时运行 `cargo audit` 与 `npm audit`);Dependabot 每周检查依赖更新。

## [0.2.1] - 2026-11-16

### 新增
- Web 界面多语言:简体中文、English、繁體中文(香港粵語)。顶部菜单切换,按浏览器语言自动选择并记住选择,切换无需刷新。
- 多语言维护工具 `scripts/i18n/i18n.mjs`:检查缺失 / 过期译文,用 Claude API 增量翻译。
- GitHub Actions "Prepare release":发布时自动翻译新增或改动的界面文字、更新版本号、打标签并发布;
  Release 工作流在译文不是最新时拒绝发布;CI 对过期译文给出警告。

### 修复
- 统计面板中数量为 0 的类型不再显示一条细线。

## [0.2.0] - 2026-11-16

### 新增
- **历史归档(orbit-archive)**:获取数据后不再只保留最新一份。
  - 根数历史库(SQLite):每组根数(编号 + 历元)只写一次,记录首次获取时间,永不覆盖;记录每次采集。
  - 完整快照:每次网络下载的原始文件压缩保存,可设最多文件数与空间上限。
  - 历史库保留控制:可设保留天数与空间上限,每个物体最新一组根数永不删除。
- **时刻策略**:`known-at`(该时刻已获取到的数据,可复现,默认)、`nearest-epoch`、`latest`;
  命令行 `--policy`、配置与 Web 界面均可切换;输出注明数据依据,每行带 `fetched_at`。
- **采集策略**:`serve` / `daemon`(`orbitscope collect`)/ `external`(系统定时任务调用 `fetch`),配置与 Web 界面可切换。
- 命令行:`collect`、`history status|fetches|object|snapshots|prune`。
- API:`GET/PUT /api/v1/settings`;`/status` 增加数据依据、设置与最近采集记录。
- Web 界面:"时间与数据"面板(实时 / 指定时刻、时刻策略、采集策略、数据依据)。
- 部署:macOS launchd 与 Linux systemd 的三种采集方式示例;Docker 增加数据卷 `/var/lib/orbitscope`。

### 变更
- 手动刷新在数据未变化时不再增加编目代数(WebSocket 客户端不必重新下载元数据)。

## [0.1.0] - 2026-11-16

首个版本。

### 新增
- **核心计算(orbit-core)**:OMM JSON(CelesTrak / Space-Track)与 TLE/3LE 解析,支持 9 位编号;
  SGP4 外推、TEME → ECEF → WGS84 坐标转换;轨道几何;基于 RCS 的等效直径区间估算;
  三种高度判据及尺寸、类型、国家、倾角、搜索、根数龄期过滤;编目去重合并;批量并行外推与地面轨迹;统计。
- **数据源(orbit-sources)**:Space-Track(登录会话、官方推荐查询、每小时上限、限速)、
  CelesTrak(分组 GP + SATCAT 元数据、条件请求)、本地文件;磁盘缓存、刷新间隔硬下限、离线模式、旧缓存回退。
- **命令行(orbitscope)**:`fetch`、`list`、`show`、`export`、`stats`、`config`、`serve`;TOML 配置。
- **Web 服务(orbit-server)**:REST API、WebSocket 二进制位置帧推送、随机分钟定时刷新、写操作鉴权、模拟时钟。
- **Web 前端**:CesiumJS 3D 地球点云、过滤/搜索/统计/数据源面板、物体详情与轨道线、手机布局。
- **打包**:Dockerfile(内置 CesiumJS)、docker-compose、systemd 服务示例、GitHub Actions(CI 与发布)。

### 已知限制
- 尺寸为 RCS 推算的估算值;Space-Track 对普通账号只提供 RCS 分档。
- CelesTrak 的默认分组只含活跃物体与几次主要解体事件的碎片;完整碎片编目需启用 Space-Track。
- 新发射物体在 CelesTrak SATCAT 中常无 RCS 数据,尺寸显示为"未知"。
