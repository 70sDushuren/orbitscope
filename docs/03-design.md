# OrbitScope 功能设计 / 架构设计

> 版本 0.1 · 2026-11-16 · 状态:草案(待评审)

## 1. 总体架构

```
            ┌───────────────────────── orbitscope (单一可执行文件) ─────────────────────────┐
            │                                                                              │
 Space-Track│  ┌──────────────┐   RawRecords   ┌──────────────┐   Catalog (Arc swap)      │
 CelesTrak ─┼─▶│ orbit-sources │──────────────▶│  orbit-core   │◀──────────────┐          │
 本地文件   │  │ 抓取/缓存/限速 │               │ 解析/合并/外推 │               │          │
            │  └──────┬───────┘               │ 坐标/尺寸/过滤 │               │          │
            │         │ 磁盘缓存               └──────┬───────┘               │          │
            │         ▼                              │ 每 tick 批量外推        │          │
            │   ~/.cache/orbitscope                  ▼                         │          │
            │                                ┌────────────────┐  Scheduler    │          │
            │                                │  orbit-server   │──(定时刷新)───┘          │
            │                                │ axum REST + WS  │                          │
            │                                └───────┬────────┘                          │
            │   CLI (clap): fetch/list/show/export/serve│                                 │
            └───────────────────────────────────────────┼─────────────────────────────────┘
                                                        │ HTTP / WebSocket(二进制帧)
                                                        ▼
                                           浏览器:CesiumJS 3D 地球 + 面板
```

### 1.1 Cargo workspace 布局

```
orbitscope/
├── Cargo.toml                 # workspace
├── crates/
│   ├── orbit-core/            # 纯计算库,无 IO、无 async
│   │   └── src/{model.rs, omm.rs, tle.rs, propagate.rs, frames.rs, elements.rs,
│   │            size.rs, filter.rs, catalog.rs, stats.rs, time.rs, error.rs}
│   ├── orbit-sources/         # 数据源 + 缓存 + 限速(async, reqwest)
│   │   └── src/{lib.rs, celestrak.rs, spacetrack.rs, file.rs, cache.rs,
│   │            satcat.rs, merge.rs, ratelimit.rs}
│   ├── orbit-server/          # axum 服务、调度器、WS 推送
│   │   └── src/{lib.rs, state.rs, scheduler.rs, api/*.rs, ws.rs, frame.rs, assets.rs}
│   └── orbitscope/            # 二进制 crate:CLI 入口
│       └── src/{main.rs, cmd/*.rs, config.rs}
├── web/                       # 前端静态资源(构建后由 rust-embed 打包进二进制)
├── config/orbitscope.example.toml
├── testdata/                  # 固定的 OMM/TLE/SATCAT 样本,测试离线可跑
└── docs/
```

依赖方向:`orbitscope → orbit-server → orbit-sources → orbit-core`;`orbit-core` 不依赖其他内部 crate。

## 2. 数据模型(orbit-core::model)

```rust
pub type NoradId = u32;                       // 支持 9 位编号

pub enum ObjectType { Payload, RocketBody, Debris, Unknown }

pub enum RcsInfo {
    Numeric(f64),                             // m²
    Category(RcsCategory),                    // Small / Medium / Large
    Unknown,
}

pub struct SizeEstimate {
    pub min_diameter_m: Option<f64>,          // 区间下界
    pub max_diameter_m: Option<f64>,          // 区间上界
    pub source: SizeSource,                   // NumericRcs / RcsCategory / Unknown
}

pub struct SpaceObject {
    pub norad_id: NoradId,
    pub name: String,
    pub intl_designator: Option<String>,      // COSPAR ID
    pub object_type: ObjectType,
    pub country: Option<String>,
    pub launch_date: Option<NaiveDate>,
    pub rcs: RcsInfo,
    pub size: SizeEstimate,                   // 由 rcs 派生
    pub elements: MeanElements,               // OMM 平根数 + epoch
    pub derived: DerivedOrbit,                // 周期、a、近/远地点高度、倾角
    pub source: DataSource,                   // 来自哪个数据源
}

pub struct Catalog {                          // 不可变快照,刷新时整体替换(ArcSwap)
    pub objects: Vec<SpaceObject>,
    pub propagators: Vec<sgp4::Constants>,    // 与 objects 下标对齐,预初始化
    pub fetched_at: DateTime<Utc>,
    pub sources: Vec<SourceStatus>,
}

pub struct StateVector {                      // 外推结果
    pub norad_id: NoradId,
    pub teme_km: [f64; 3], pub teme_vel_kms: [f64; 3],
    pub ecef_km: [f64; 3],
    pub lat_deg: f64, pub lon_deg: f64, pub alt_km: f64,
}
```

设计要点:
- `Catalog` 是**不可变快照**,刷新数据时构建新快照并原子替换(`arc-swap`),读者无锁。
- SGP4 `Constants` 在构建快照时一次性初始化,外推时只做 `propagate(t)`。
- 过滤分两级:**静态过滤**(类型、尺寸、轨道高度区间等,只依赖根数)在快照/过滤条件变化时计算一次,得到下标列表;
  **动态过滤**(瞬时高度模式)在每次外推后执行。

## 3. 核心算法

### 3.1 外推与坐标转换
1. `minutes_since_epoch = (t - epoch)` → `sgp4::Constants::propagate` → TEME 位置/速度(km, km/s)。
2. TEME → ECEF(PEF):绕 Z 轴旋转 GMST(IAU-1982 公式,用 UT1≈UTC);忽略极移(误差约 10 m 级,远小于 SGP4 误差)。
3. ECEF → WGS84 大地坐标:Bowring 迭代或 Zhu/Heikkinen 闭式解。
4. 批量外推用 `rayon` 并行,输出到预分配的 `Vec<StateVector>` / 紧凑 `Vec<[f32; 4]>`。

### 3.2 轨道几何(只依赖根数)
- `n` = 平均运动(rev/day)→ `a = (μ / n²)^(1/3)`(μ = 398600.4418 km³/s²,n 转 rad/s)
- 近地点高度 `hp = a(1−e) − Re`,远地点高度 `ha = a(1+e) − Re`,`Re = 6378.137 km`
- 周期 `T = 1440 / n` 分钟。Space-Track 的 `PERIAPSIS/APOAPSIS` 字段可用于交叉校验。

### 3.3 高度过滤模式
| 模式 | 判定 | 场景 |
|------|------|------|
| `orbit-overlap`(默认) | `hp ≤ max && ha ≥ min` | 会穿越该高度带的所有物体 |
| `orbit-within` | `hp ≥ min && ha ≤ max` | 完全处于该高度带内的物体 |
| `instantaneous` | `min ≤ alt(t) ≤ max` | 此刻在该高度带内的物体 |

### 3.4 尺寸估算(orbit-core::size)
- 数值 RCS:`d = 2·√(RCS/π)`(光学区球体近似)。接口设计为 `trait SizeModel`,后续可替换为 NASA SEM 曲线。
- 分档:SMALL → `[None, 0.357 m]`,MEDIUM → `[0.357, 1.128]`,LARGE → `[1.128, None]`。
- 判定 `passes_min_size(min_d)`:
  - `min_diameter ≥ min_d` → 通过;`max_diameter < min_d` → 不通过;
  - 其余(区间跨越门限或未知)→ 按 `unknown_size` 策略(include/exclude)。

### 3.5 数据合并
1. 各数据源产出 `RawGp`(OMM 字段集)与可选 `RawSatcat`。
2. 以 NORAD 编号为键:GP 取 epoch 最新者;元数据按优先级 Space-Track GP 字段 > CelesTrak SATCAT > 未知。
3. 剔除:有 `DECAY_DATE` 的、epoch 龄期 > `max_epoch_age_days` 的、SGP4 初始化失败的(记录到状态统计)。

## 4. 数据源设计(orbit-sources)

数据源用枚举 `Source { SpaceTrack, CelesTrak, File }` 统一调度(见 ADR-011),由 `SourceSet` 按优先级依次获取:

```rust
impl SourceSet {
    pub fn from_config(cfg: &SourcesConfig, creds: Option<Credentials>) -> Result<Self, SourceError>;
    pub async fn fetch_all(&self, mode: FetchMode, now: DateTime<Utc>) -> FetchAllResult;
}
impl FetchAllResult {
    pub fn build_catalog(self, now) -> (Catalog, BuildReport);   // 合并 SATCAT 元数据后构建编目
}
```

| 数据源 | 认证 | 内容 | 刷新策略 |
|-------|------|------|---------|
| **Space-Track** | 账号(cookie 会话:`POST /ajaxauth/login`,表单 `identity` / `password`;401 时自动重登一次) | `class/gp/decay_date/null-val/epoch/%3Enow-10/PERIAPSIS/%3C2000/orderby/norad_cat_id/format/json`;含 OBJECT_TYPE、RCS_SIZE、COUNTRY_CODE 等,无需另取 SATCAT。`PERIAPSIS` 预筛可配置(默认 2000 km,减少下载量) | **硬下限 1 小时**(官方规定);限速默认 20 次/分、200 次/时(官方上限 30 / 300,代码中强制钳制在上限以下);随机分钟调度在 M4 实现 |
| **CelesTrak GP** | 无 | `/NORAD/elements/gp.php?GROUP=<g>&FORMAT=json`;默认分组 `active`、`cosmos-1408-debris`、`fengyun-1c-debris`、`iridium-33-debris`、`cosmos-2251-debris` | 默认 120 分钟,硬下限 60 分钟;ETag / Last-Modified 条件请求,304 时沿用缓存 |
| **CelesTrak SATCAT** | 无 | `/pub/satcat.csv`(OBJECT_TYPE、OWNER、数值 RCS m²、DECAY_DATE),按编号补全 GP 记录元数据 | 默认 1 天,硬下限 12 小时 |
| **文件** | 无 | TLE / 3LE / OMM JSON(按内容识别) | 每次获取时读取 |

获取流程(`http::cached_fetch`):

1. `CacheOnly`:只读缓存,无缓存则报 `NoCache`;
2. `Normal`:缓存年龄 < 刷新间隔 → 直接用缓存(**程序重启后同样生效**,保证不违反频率规定);
3. 否则联网(带条件请求头),失败最多重试 1 次(5xx / 429 / 网络错误,尊重 `Retry-After`);
4. 联网失败但有旧缓存 → 使用旧缓存并标记 `StaleCache`;单个分组/文件失败只记入 `notes`,全部失败才报错。

缓存:`<cache_dir>/<key>.body` + `<key>.meta.json`(URL、fetched_at、ETag、Last-Modified、字节数),临时文件 + rename 原子写入。
默认 `cache_dir` 为用户缓存目录下的 `orbitscope`。

所有 HTTP 请求带统一 `User-Agent: orbitscope/<ver>`,不附带任何个人信息。`Credentials` 的 `Debug` 输出不含密码,
登录错误信息不含凭据(有测试覆盖)。

> **已核实(2026-11)**:Space-Track 官方文档中的限速、GP 每小时 1 次及推荐查询条件。
> Space-Track 的 `RCSVALUE` 对普通账号恒为 0,且自 2026-05-28 起单位改为"米",因此不读取该字段,只用 `RCS_SIZE` 分档。
> **未能核实**:CelesTrak 在本项目的云端开发环境中返回 500 / 连接被重置(疑似拒绝数据中心 IP),
> 分组名与 SATCAT 路径依据其公开文档格式实现,均可在配置中修改,首次在本地运行时需确认。

## 5. 服务端设计(orbit-server)

### 5.1 运行时任务
- **Scheduler**:按各数据源间隔刷新 → 构建新 `Catalog` → ArcSwap 替换 → 广播 `CatalogUpdated` 事件。
- **Ticker**:每 `tick_interval`(默认 1 s)对"任一订阅者需要的物体并集"做一次批量外推,结果放入共享缓冲,各 WS 连接按自己的过滤下标集合取子集编码发送。
- **WS 连接任务**:每连接一个 task,慢客户端丢帧(只保留最新帧,`tokio::sync::watch`)。

### 5.2 REST API(前缀 `/api/v1`)
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/status` | 版本、数据源状态、最后更新时间、物体总数 |
| GET | `/filters/default` | 服务器默认过滤条件 |
| PUT | `/filters/default` | 修改默认过滤(需 token,若启用) |
| GET | `/objects?alt_min=&alt_max=&min_size_cm=&types=&q=&page=&per_page=&sort=` | 物体列表(元数据 + 当前位置) |
| GET | `/objects/{norad_id}` | 详情(根数、派生轨道参数、尺寸估算、当前状态) |
| GET | `/objects/{norad_id}/track?before_min=&after_min=&step_s=` | 轨道线 / 地面轨迹点 |
| GET | `/stats?<同过滤参数>` | 按类型/国家计数、高度直方图 |
| POST | `/refresh` | 手动触发刷新(需 token,受限速约束) |

### 5.3 WebSocket 协议(`/api/v1/stream`)
- 客户端 → 服务端(JSON 文本):`{"type":"subscribe","filter":{...},"rate_hz":1}`;可随时重新 subscribe。
- 服务端 → 客户端:
  - JSON `{"type":"catalog","objects":[{id,name,type,...}]}`:订阅/数据更新时发送一次元数据(含下标→ID 映射)。
  - 二进制位置帧(小端):
    ```
    u32 magic 'OSF1' | u32 version | f64 unix_time_s | u32 count |
    count × { u32 index ; f32 x_km ; f32 y_km ; f32 z_km }   // ECEF,16 字节/物体
    ```
    前端用 ECEF 直接给 Cesium(Cartesian3 单位 m,×1000),并在两帧之间线性插值或用速度外推以获得平滑动画。

## 6. 前端设计(web/)

- **CesiumJS**,使用 `TileMapServiceImageryProvider` 加载其自带的 NaturalEarthII 离线影像 → 无需 ion token、可离线。
- 物体渲染:`PointPrimitiveCollection`(2–3 万点性能良好);颜色:载荷 绿、火箭体 橙、碎片 红、未知 灰。
- 选中物体:Polyline 显示 ±1 个周期轨道线 + 地面轨迹;信息面板显示详情。
- 面板:过滤(高度双滑块、尺寸、类型复选、搜索框)、统计(计数 + 高度直方图)、状态栏。
- 构建:原生 ES Module + 少量 TypeScript(可选 esbuild),产物由 `rust-embed` 打包进二进制;开发模式从磁盘读取。

## 7. 配置

优先级:CLI 参数 > 环境变量(`ORBITSCOPE_*`)> 配置文件 > 默认值。示例见 `config/orbitscope.example.toml`:

```toml
[filter]
alt_min_km = 100.0
alt_max_km = 1000.0
altitude_mode = "orbit-overlap"      # orbit-overlap | orbit-within | instantaneous
min_size_cm = 10.0
unknown_size = "include"             # include | exclude
object_types = ["PAYLOAD", "ROCKET BODY", "DEBRIS", "UNKNOWN"]
max_epoch_age_days = 14

[sources.celestrak]
enabled = true
groups = ["active"]                  # M2 时核实并补充碎片分组
refresh_minutes = 120

[sources.spacetrack]
enabled = false                      # 凭据:ORBITSCOPE_SPACETRACK_USER / ORBITSCOPE_SPACETRACK_PASSWORD
refresh_minutes = 120

[server]
bind = "127.0.0.1:8080"
tick_ms = 1000
```

## 8. 错误处理与日志
- 库 crate 用 `thiserror` 定义错误枚举;二进制用 `anyhow` 汇总上下文。
- 单物体错误(解析/外推)计数并记录 `debug` 日志,不中断批处理。
- `tracing` 结构化日志;凭据字段实现自定义 `Debug` 为 `***`。

## 9. 测试策略
| 层级 | 内容 |
|------|------|
| 单元 | 根数 → 近/远地点;GMST;TEME→ECEF→LLA(对照 Vallado 例题或 Skyfield 生成的参考值,存于 `testdata/`);尺寸区间判定;过滤模式 |
| 解析 | OMM JSON / TLE / SATCAT CSV 样本(含 9 位编号、缺失字段) |
| 数据源 | `wiremock` 模拟 HTTP;限速、退避、缓存命中;**测试绝不访问真实数据源** |
| 服务 | axum `oneshot` 测 REST;WS 帧编解码往返测试 |
| 基准 | `criterion`:3 万物体批量外推 + 转换 |

## 10. 架构决策记录(ADR)

| ADR | 决策 | 备选 | 理由 |
|-----|------|------|------|
| 001 | **服务端外推 + 浏览器渲染** | 原生 Bevy/egui 桌面端;TUI | Web 3D 地球生态(Cesium)最成熟、跨平台、可多人访问;Rust 负责全部数据与计算。桌面端/TUI 留作 P2 |
| 002 | 外推用 `sgp4` crate | `satkit`、自研 | 纯 Rust、经验证、原生支持 OMM JSON;`satkit` 功能更多但依赖重 |
| 003 | OMM JSON 为内部主格式 | TLE | 9 位编号即将启用,TLE 无法表示 |
| 004 | Catalog 不可变快照 + ArcSwap | `RwLock<Catalog>` | 读多写少,读路径无锁 |
| 005 | WS 二进制帧,ECEF f32 | JSON、经纬高 | 体积小(16B/物体),前端无需再转换;f32 在 7000 km 量级精度约 0.5 m,足够 |
| 006 | 前端 CesiumJS + 离线影像 | three.js / globe.gl / MapLibre | 内建地球、相机、时间系统与大量点渲染;离线影像免 token |
| 007 | 尺寸 = RCS 等效直径估算,区间化 | 只用分档 / 忽略尺寸 | 公开数据只有 RCS;区间化可诚实表达不确定性 |
| 008 | 默认数据源 CelesTrak,Space-Track 可选 | 只用 Space-Track | 免注册即可开箱运行;需要全量碎片时再配账号 |
| 009 | SGP4 使用 sgp4 crate 默认(改进)模式 | AFSPC 兼容模式 | crate 推荐;与 AFSPC 参考向量差异仅数十米,远小于 SGP4 的 km 级误差 |
| 010 | 根数龄期过滤放在 `Filter`(运行时可调),编目构建只剔除已再入与非法根数 | 构建时剔除过期根数 | 修改龄期阈值无需重建编目 |
| 011 | 数据源用枚举 `Source` 而非 `dyn Trait` | `async_trait` + trait 对象 | 数据源种类固定且少;避免额外依赖与装箱 future |
| 012 | 刷新间隔设硬下限,且以磁盘缓存时间为准 | 只在内存中记录上次抓取时间 | 程序反复重启也不会违反数据源频率规定 |
