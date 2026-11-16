//! 应用配置:TOML 文件 + 命令行覆盖。
//!
//! 查找顺序(取第一个存在的):
//! 1. `--config <路径>` / 环境变量 `ORBITSCOPE_CONFIG`(指定后文件必须存在)
//! 2. 当前目录 `./orbitscope.toml`
//! 3. 用户配置目录 `<config_dir>/orbitscope/config.toml`
//! 4. 全部使用默认值
//!
//! 凭据**不**放在配置文件中,只从环境变量读取。

use anyhow::{Context, Result, bail};
use orbit_core::Filter;
use orbit_sources::SourcesConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 示例配置(随二进制分发,`orbitscope config init` 输出)。
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config/orbitscope.example.toml");

/// 服务端配置(M4 使用)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub bind: String,
    pub tick_ms: u64,
    pub max_ws_clients: usize,
    /// 前端加载 CesiumJS 的地址(以 / 结尾)
    pub cesium_url: String,
    /// 本地 CesiumJS `Build/Cesium` 目录;设置后前端可完全离线
    pub cesium_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".into(),
            tick_ms: 1_000,
            max_ws_clients: 64,
            cesium_url: orbit_server::DEFAULT_CESIUM_URL.to_string(),
            cesium_dir: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub filter: Filter,
    pub sources: SourcesConfig,
    pub server: ServerConfig,
}

/// 加载结果:配置 + 来源路径(使用默认值时为 `None`)。
#[derive(Debug)]
pub struct Loaded {
    pub config: AppConfig,
    pub path: Option<PathBuf>,
}

impl AppConfig {
    pub fn from_toml(text: &str) -> Result<Self> {
        let cfg: AppConfig = toml::from_str(text)?;
        cfg.filter.validate().context("[filter] 段无效")?;
        Ok(cfg)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("读取配置文件 {}", path.display()))?;
        Self::from_toml(&text).with_context(|| format!("解析配置文件 {}", path.display()))
    }
}

/// 默认的用户配置文件路径。
pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("orbitscope").join("config.toml"))
}

/// 按查找顺序加载配置。
pub fn load(explicit: Option<&Path>) -> Result<Loaded> {
    if let Some(p) = explicit {
        if !p.exists() {
            bail!("配置文件不存在: {}", p.display());
        }
        return Ok(Loaded {
            config: AppConfig::from_file(p)?,
            path: Some(p.to_path_buf()),
        });
    }
    let candidates = [Some(PathBuf::from("orbitscope.toml")), user_config_path()];
    for p in candidates.into_iter().flatten() {
        if p.is_file() {
            return Ok(Loaded {
                config: AppConfig::from_file(&p)?,
                path: Some(p),
            });
        }
    }
    Ok(Loaded {
        config: AppConfig::default(),
        path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses_and_matches_defaults() {
        let cfg = AppConfig::from_toml(EXAMPLE_CONFIG).unwrap();
        // 示例文件中的取值应与代码默认值一致,避免文档与实现脱节
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn partial_config() {
        let cfg =
            AppConfig::from_toml("[filter]\nalt_max_km = 600\n[sources.celestrak]\ngroups = [\"active\"]\n")
                .unwrap();
        assert_eq!(cfg.filter.alt_max_km, 600.0);
        assert_eq!(cfg.filter.alt_min_km, 100.0);
        assert_eq!(cfg.sources.celestrak.groups, vec!["active"]);
    }

    #[test]
    fn rejects_unknown_keys_and_invalid_filter() {
        assert!(AppConfig::from_toml("[filter]\nalt_maxkm = 1\n").is_err());
        assert!(AppConfig::from_toml("[filter]\nalt_min_km = 900\nalt_max_km = 100\n").is_err());
        assert!(
            AppConfig::from_toml("[sources.spacetrack]\npassword = \"x\"\n").is_err(),
            "凭据不允许写入配置"
        );
    }
}
