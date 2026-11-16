//! 静态资源:内嵌的前端(`web/` 目录,编译进二进制)与可选的本地 CesiumJS 目录。
//!
//! debug 构建时 rust-embed 直接从磁盘读取 `web/`,修改前端无需重新编译。

use crate::state::{AppState, ServerOptions};
use axum::Router;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(RustEmbed)]
#[folder = "../../web/"]
pub struct WebAssets;

/// 前端与 Cesium 路由。
pub fn routes(options: &ServerOptions) -> Router<Arc<AppState>> {
    let mut r = Router::new().fallback(static_file);
    if let Some(dir) = &options.cesium_dir {
        r = r.nest_service("/cesium", ServeDir::new(dir));
    }
    r
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // API 路径不回落到前端
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "未知的 API 路径" })),
        )
            .into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    match WebAssets::get(path) {
        Some(file) => {
            let mime = file.metadata.mimetype().to_string();
            // index.html 不缓存,其余资源短缓存(无内容哈希文件名)
            let cache = if path == "index.html" {
                "no-cache"
            } else {
                "public, max-age=300"
            };
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
