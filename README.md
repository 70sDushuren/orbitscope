# OrbitScope

准实时近地轨道空间物体监视程序(Rust)。

- 数据源:Space-Track(需账号,全量编目)/ CelesTrak(免账号)/ 本地 TLE、OMM 文件
- SGP4 本地外推,位置每秒刷新
- 过滤:高度范围(默认 100–1000 km)、最小尺寸(默认 10 cm,基于 RCS 估算)、类型、国家、搜索
- 展示:Web 3D 地球(CesiumJS)、列表、详情与轨迹、统计;CLI 与 REST/WebSocket API

> 状态:规划阶段(M0)。见 [docs/](docs/)。

| 文档 | 内容 |
|------|------|
| [01-project-plan.md](docs/01-project-plan.md) | 愿景、范围、里程碑、风险 |
| [02-requirements.md](docs/02-requirements.md) | 功能与非功能需求 |
| [03-design.md](docs/03-design.md) | 架构、数据模型、算法、API、ADR |
| [04-references.md](docs/04-references.md) | 开源软件与公开数据服务调研 |

**关于尺寸**:公开编目不包含物体物理尺寸,本程序的尺寸为雷达截面积(RCS)推算的等效直径估算值。
美国空间监视网在 LEO 的编目门限约 10 cm,公开编目基本即为 ≥10 cm 物体。
