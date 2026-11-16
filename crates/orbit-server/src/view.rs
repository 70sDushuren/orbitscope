//! 各连接的"指定时刻"视图:按视图时刻构建编目。
//!
//! 视图的编目与服务端实时编目用同一套规则选取(历史库 + 当前时刻策略),只是时刻不同;
//! 没有历史库时内容与实时编目相同,只有时钟不同。

use crate::state::{AppState, CatalogState, catalog_signature};
use chrono::{DateTime, Utc};
use orbit_core::Catalog;
use std::sync::Arc;

/// 构建视图在 `at` 时刻的编目。
///
/// 内容与 `prev` 相同时沿用其代数(客户端无需重新下载元数据),否则分配新的代数。
pub async fn build_catalog(
    state: &AppState,
    at: DateTime<Utc>,
    prev: Option<&Arc<CatalogState>>,
) -> Arc<CatalogState> {
    let policy = state.settings().time_policy;
    let archive = state.options.archive.clone().filter(|a| a.history().is_some());
    let (catalog, report, provenance, signature) = match archive {
        Some(a) => match tokio::task::spawn_blocking(move || a.catalog_at(policy, at)).await {
            Ok(Ok(Some((catalog, report, prov)))) => {
                let signature = catalog_signature(&catalog) ^ (policy as u64).wrapping_mul(0x9E37_79B9);
                (catalog, report, Some(prov), signature)
            }
            other => {
                match other {
                    Ok(Err(e)) => tracing::error!(error = %e, "读取历史库失败"),
                    Err(e) => tracing::error!(error = %e, "读取历史库任务失败"),
                    _ => {}
                }
                let (catalog, report) = Catalog::build(Vec::new(), at);
                (catalog, report, None, 0)
            }
        },
        None => {
            let live = state.catalog.load_full();
            (live.catalog.clone(), live.report.clone(), None, live.signature)
        }
    };
    let (generation, loaded_at) = match prev {
        Some(p) if p.signature == signature => {
            if p.provenance == provenance {
                return p.clone();
            }
            (p.generation, p.loaded_at)
        }
        _ => (state.next_view_generation(), Utc::now()),
    };
    Arc::new(CatalogState {
        catalog,
        generation,
        loaded_at,
        report,
        provenance,
        signature,
    })
}
