# 调研:现有开源软件与公开服务

> 2026-11-16 调研。标注"待核实"的条目在规划环境中无法直接访问官网验证,编码对应模块前需再次确认。

## 1. 公开数据服务

| 服务 | 内容 | 访问方式 | 备注 |
|------|------|---------|------|
| **Space-Track.org**(美国太空军 / 18th SDS) | 权威公开编目:GP(OMM/TLE)、SATCAT、衰减/再入、gp_history | 免费注册账号,REST 查询语言 `basicspacedata/query/class/...` | `RCS_SIZE` 自 2014 年起仅为分档:SMALL < 0.1 m²、MEDIUM 0.1–1 m²、LARGE > 1 m²。限速常见引用为 30 次/分、300 次/时;GP 全量建议 ≤ 1 次/小时;gp_history 有更严格限制(待核实) |
| **CelesTrak**(T.S. Kelso) | GP 数据(按分组/编号/名称查询)、SATCAT、SupGP(运营方提供的补充星历) | 无需账号:`/NORAD/elements/gp.php?{CATNR|INTDES|GROUP|NAME|SPECIAL}=..&FORMAT=json|tle|xml|csv|kvn` | JSON/OMM 支持 9 位编号,TLE 只支持 5 位;SATCAT 含数值 RCS(m²,可能为空,早期值不可靠);有使用政策,按 IP 统计、过度下载会被封(具体阈值待核实) |
| ESA DISCOS | 物体物理属性(尺寸、质量、形状) | 需申请账号 | 可作为未来"真实尺寸"数据源(P2) |
| ESA / NASA ODPO 报告 | 碎片环境统计、尺寸估算模型(SEM) | 公开文档 | 尺寸估算算法依据 |
| GCAT(J. McDowell) | 独立整理的全编目,含尺寸/质量估计 | 公开 TSV | 可作为尺寸补充数据源(P2,需注意许可) |

## 2. 开源可视化/跟踪软件

| 项目 | 技术 | 可借鉴点 |
|------|------|---------|
| **KeepTrack**(keeptrack.space) | TypeScript + WebGL | 大规模(数万)物体实时渲染、Web Worker 外推、搜索与过滤 UI |
| **Stuff in Space** | JS + WebGL | 极简的全编目点云展示,按类型着色 |
| **satvis** | Vue + CesiumJS + satellite.js | Cesium 中的卫星、轨道线、地面站过境 |
| **CesiumJS** | WebGL 地球引擎 | 离线 NaturalEarthII 影像、PointPrimitiveCollection |
| **Gpredict** | C / GTK 桌面 | 过境预报、多视图(地图/极坐标/列表)设计 |
| **satellite.js** | JS SGP4 | 前端外推参考(若将来走 WASM 方案可对照) |
| **Skyfield**(Python) | 天文/坐标库 | 生成测试参考值(TEME→ITRS→WGS84) |

## 3. Rust 生态

| crate | 用途 | 备注 |
|-------|------|------|
| `sgp4`(neuromorphicsystems) | SGP4/SDP4 外推 | 纯 Rust,支持 TLE 与 OMM(serde,需 `alloc`),最新 2.x;仓库有从 CelesTrak 拉 OMM 的示例 |
| `satkit` | 天体力学工具箱(含 SGP4、坐标系、高精度外推) | 功能全、依赖重;坐标转换可参考 |
| `brahe` | 航天动力学库,内置 CelesTrak/Space-Track 客户端 | 客户端实现(限速、缓存)可参考 |
| `nyx-space` / `anise` | 高精度轨道动力学 | 超出本项目精度需求 |
| `axum` / `tokio` / `reqwest` | Web 服务、异步、HTTP | — |
| `rayon` | 数据并行 | 批量外推 |
| `arc-swap` | 无锁快照替换 | Catalog 刷新 |
| `rust-embed` | 静态资源打包 | 前端打进单一二进制 |
| `ratatui` | TUI | P2 |
| `bevy` / `egui` | 原生 3D / GUI | P2 桌面端备选 |

## 4. 参考链接

- Space-Track:https://www.space-track.org/documentation
- CelesTrak GP 数据格式说明:https://celestrak.org/NORAD/documentation/gp-data-formats.php
- CelesTrak 使用政策:https://celestrak.org/usage-policy.php
- CelesTrak SATCAT 格式:https://celestrak.org/satcat/satcat-format.php
- sgp4 crate:https://docs.rs/sgp4 · https://github.com/neuromorphicsystems/sgp4
- satkit:https://docs.rs/satkit
- brahe(CelesTrak / Space-Track 客户端与限速说明):https://duncaneddy.github.io/brahe/latest/learn/ephemeris/spacetrack/index.html
- Space-Track 限速(第三方客户端整理):https://spacetrack.readthedocs.io/en/latest/changelog.html
- RCS 分档说明:https://metacpan.org/pod/release/WYANT/Astro-SpaceTrack-0.087/lib/Astro/SpaceTrack.pm
- NASA ODPO 尺寸估算:https://ntrs.nasa.gov/citations/20250002268
- Vallado et al., *Revisiting Spacetrack Report #3*(SGP4 参考实现与测试向量)
