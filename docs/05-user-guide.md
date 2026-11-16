# OrbitScope 使用手册

> 适用版本 0.1.0。设计细节见 [03-design.md](03-design.md)。

## 1. 安装

### 1.1 预编译二进制
在 GitHub Releases 页面下载对应平台的压缩包(Linux x86_64、macOS Apple Silicon、Windows x86_64),
解压后得到 `orbitscope` 可执行文件、示例配置与许可证。

### 1.2 从源码编译
需要 Rust 1.88 或更新版本:

```bash
git clone https://github.com/70sDushuren/orbitscope
cd orbitscope
cargo build --release
./target/release/orbitscope --version
```

### 1.3 Docker
镜像内置 CesiumJS,浏览器无需访问 CDN:

```bash
docker compose up -d                  # 使用仓库中的 docker-compose.yml
# 或
docker build -t orbitscope .
docker run -d -p 127.0.0.1:8080:8080 -v orbitscope-cache:/var/cache/orbitscope orbitscope
```

自定义配置:`-v ./my.toml:/etc/orbitscope/config.toml:ro`(容器内需保留 `bind = "0.0.0.0:8080"` 与
`cesium_dir = "/opt/cesium"`,参考 `docker/config.toml`)。

### 1.4 systemd
见 `deploy/orbitscope.service` 文件头部的步骤。

## 2. 数据源

| 数据源 | 是否需要账号 | 内容 | 说明 |
|-------|------------|------|------|
| CelesTrak | 否 | 活跃物体 + 几次主要解体事件的碎片,另取 SATCAT 补充类型、国家与数值 RCS | 默认启用;不含全部碎片 |
| Space-Track | 是(免费注册) | 完整公开编目,含全部碎片、类型、国家、RCS 分档 | 默认关闭 |
| 本地文件 | 否 | TLE / 3LE / OMM JSON | 适合离线或自有数据 |

### 2.1 启用 Space-Track
1. 在 https://www.space-track.org 注册账号;
2. 配置文件中设置:
   ```toml
   [sources.spacetrack]
   enabled = true
   ```
3. 通过环境变量提供凭据(**不要写进配置文件**,程序会拒绝):
   ```bash
   export ORBITSCOPE_SPACETRACK_USER='账号'
   export ORBITSCOPE_SPACETRACK_PASSWORD='密码'
   ```

### 2.2 刷新频率与缓存
- 下载结果缓存在用户缓存目录下的 `orbitscope/`(Linux 为 `~/.cache/orbitscope`),可用 `[sources] cache_dir` 修改;
- 每个数据源都有最短刷新间隔(Space-Track 60 分钟,CelesTrak 60 分钟,SATCAT 12 小时),
  间隔内无论运行多少次命令或重启多少次服务,都只使用缓存;
- 服务模式下每小时在随机分钟检查一次更新(避开整点/半点,符合 Space-Track 规定);
- 联网失败时自动使用旧缓存,并在状态中标注;`--offline` 只用缓存。

## 3. 过滤条件

| 配置项(`[filter]`) | 命令行 | 说明 |
|--------------------|-------|------|
| `alt_min_km` / `alt_max_km` | `--alt 100..1000` 或 `--alt-min` / `--alt-max` | 高度范围(km),默认 100–1000 |
| `altitude_mode` | `--alt-mode` | `orbit-overlap`(默认):轨道[近地点,远地点]与范围有交集;`orbit-within`:轨道完全在范围内;`instantaneous`:此刻高度在范围内 |
| `min_size_cm` | `--min-size` | 最小等效直径(cm),默认 10;0 表示不过滤 |
| `unknown_size` | `--unknown-size` | 尺寸无法判定时 `include`(默认)或 `exclude` |
| `object_types` | `--type`(可重复) | `PAYLOAD`、`ROCKET BODY`、`DEBRIS`、`UNKNOWN`;空为全部 |
| `countries` | `--country`(可重复) | 国家/所有者代码,如 `US`、`PRC`、`CIS` |
| `inclination_min_deg` / `_max_deg` | `--inc-min` / `--inc-max` | 倾角范围 |
| `query` | `-q` | 名称子串 / NORAD 编号 / 国际编号前缀 |
| `max_epoch_age_days` | `--max-age` / `--any-age` | 根数龄期上限(天),默认 14 |

### 关于"尺寸"
公开编目**不包含物体的物理尺寸**。程序用雷达截面积(RCS)推算等效直径:
数值 RCS 按 `d = 2·√(RCS/π)` 换算;Space-Track 只给分档(SMALL < 0.1 m²、MEDIUM 0.1–1 m²、LARGE > 1 m²),
换算为直径区间(< 0.36 m、0.36–1.13 m、> 1.13 m)。门限落在区间内部或没有 RCS 时为"无法判定",
由 `unknown_size` 决定是否显示。美国空间监视网在近地轨道的编目门限约为 10 cm,因此把门限调到 10 cm 以下不会得到更多物体。

## 4. 命令行

```bash
orbitscope fetch                                      # 获取并缓存数据
orbitscope list                                       # 列出(默认前 50 条)
orbitscope list --alt 400..450 --type debris --sort alt --desc -n 20
orbitscope list --alt-mode instantaneous --at +30m    # 30 分钟后此刻高度在范围内的物体
orbitscope list -f json | jq '.[0]'                   # JSON / CSV 输出
orbitscope show 25544 --track 90m --step 5m           # 详情与地面轨迹
orbitscope export -f csv -o leo.csv                   # 导出全部符合条件的物体
orbitscope stats --bin 100                            # 统计
orbitscope config init                                # 生成带注释的配置文件
orbitscope config show                                # 显示生效配置
orbitscope serve                                      # Web 服务
```

全局参数:`-c/--config <文件>`(或环境变量 `ORBITSCOPE_CONFIG`)、`--offline`、`--quiet`、`-v`/`-vv`。
时间参数 `--at`:`now`、RFC3339(如 `2026-11-16T08:00:00Z`)、相对偏移(`+90m`、`-2h`、`+1d`)。
数据源状态输出到 stderr,数据输出到 stdout,可直接用管道处理。

配置文件查找顺序:`--config` → `./orbitscope.toml` → 用户配置目录 `orbitscope/config.toml`(Linux 为 `~/.config/orbitscope/config.toml`)→ 默认值。

## 5. Web 界面

`orbitscope serve` 后在浏览器打开 http://127.0.0.1:8080/ 。

- **3D 地球**:按类型着色的点云,每秒更新、帧间平滑插值;日夜分界随时间变化。
- **左侧面板**:过滤(修改后点"应用",条件会记在浏览器中)、搜索、按类型统计、平均高度直方图、数据源状态。
- **详情**:点击点或搜索结果,显示实时经纬高、速度、轨道参数、估算尺寸,以及一个周期的空间轨迹(实线)和星下点轨迹(虚线)。
- **手机**:面板为底部抽屉,用右上角"面板"按钮切换。

`serve` 参数:`--bind 0.0.0.0:8080`(覆盖配置)、`--sim-time <时刻>`(模拟时钟,用于回放旧数据或演示)。

### 5.1 CesiumJS 与离线使用
浏览器端 3D 引擎 CesiumJS 默认从 jsDelivr 加载固定版本(`server.cesium_url`)。
完全离线时下载后配置 `server.cesium_dir`:

```bash
npm pack cesium@1.146.0 && tar -xzf cesium-1.146.0.tgz
# [server] cesium_dir = "/绝对路径/package/Build/Cesium"
```

Docker 镜像已内置。

### 5.2 对外开放
默认只监听 `127.0.0.1`。对外开放时:
- 设置 `bind = "0.0.0.0:8080"`(或放在反向代理之后);
- 设置环境变量 `ORBITSCOPE_API_TOKEN`(≥ 16 字符)。写接口(修改默认过滤、手动刷新)需要
  `Authorization: Bearer <token>`;未设置 token 时写接口只接受本机请求。读接口与 WebSocket 无需鉴权。
- 经反向代理(Nginx 等)时需转发 WebSocket(`Upgrade` / `Connection` 头)。

## 6. HTTP API

前缀 `/api/v1`,全部为 JSON;错误为 `{"error": "..."}`。

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/status` | 服务状态、编目数量与代数、数据源状态、刷新计划 |
| GET | `/objects` | 列表:过滤参数 + `at`、`page`、`per_page`(≤ 5000)、`sort`、`desc` |
| GET | `/objects/{norad}` | 单个物体详情与 `at` 时刻状态 |
| GET | `/objects/{norad}/track` | 轨迹:`at`、`before_min`、`after_min`、`step_s` |
| GET | `/stats` | 统计:过滤参数 + `bin_km` |
| GET/PUT | `/filters/default` | 默认过滤条件(PUT 为局部 JSON 合并,写操作) |
| POST | `/refresh` | 手动触发刷新(写操作;仍遵守数据源最短间隔) |
| GET | `/client-config` | 前端配置(Cesium 地址等) |
| WS | `/stream` | 实时位置推送 |

列表/统计的过滤查询参数:`alt_min`、`alt_max`、`alt_mode`、`min_size_cm`、`unknown_size`、
`types`(逗号分隔:`payload,rocket-body,debris,unknown`)、`countries`、`inc_min`、`inc_max`、`q`、`max_age_days`、`any_age`。

示例:

```bash
curl 'http://127.0.0.1:8080/api/v1/objects?alt_min=400&alt_max=450&types=debris&sort=alt&per_page=10'
curl 'http://127.0.0.1:8080/api/v1/objects/25544/track?after_min=90&step_s=60'
curl -X PUT -H 'Content-Type: application/json' -d '{"alt_max_km": 600}' http://127.0.0.1:8080/api/v1/filters/default
```

WebSocket 协议(订阅消息、元数据消息、二进制位置帧格式)见 [03-design.md](03-design.md) 第 5.3 节;
Rust 解码实现在 `crates/orbit-server/src/frame.rs`,JavaScript 实现在 `web/js/stream.js`。

## 7. 常见问题

| 现象 | 处理 |
|------|------|
| `所有数据源均失败` | 检查网络;`-v` 查看详细日志;离线时先在联网环境运行一次 `orbitscope fetch` |
| 列表为空 | 检查根数龄期(`--any-age`)、高度与尺寸条件;`orbitscope stats` 查看整体分布 |
| 浏览器提示无法加载 CesiumJS | 浏览器无法访问 jsDelivr,配置 `server.cesium_dir` 使用本地副本 |
| Space-Track 登录失败 | 检查环境变量;账号是否被临时停用(超频会被停用) |
| `cesium_dir 中找不到 Cesium.js` | 路径应指向 `package/Build/Cesium`(其中直接包含 `Cesium.js`) |
| 写接口返回 403 / 401 | 未设置 token 时只能本机调用;设置 `ORBITSCOPE_API_TOKEN` 并携带 Bearer 头 |
