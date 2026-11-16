# 更新日志

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),版本号遵循语义化版本。

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
