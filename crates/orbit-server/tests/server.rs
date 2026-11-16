//! 服务端集成测试:在随机端口启动真实服务,只使用本地文件数据源。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use orbit_core::Filter;
use orbit_server::{ServerOptions, frame, serve_with_listener};
use orbit_sources::file::{FileConfig, FileSource};
use orbit_sources::{Cache, Source, SourceSet};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// 样本根数的时刻;服务端时钟偏移到这里,使外推结果有意义。
const AT: &str = "2024-01-01T12:30:00Z";

struct TestServer {
    addr: SocketAddr,
    client: reqwest::Client,
    stop: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<Result<(), orbit_server::ServerError>>>,
    _dir: tempfile::TempDir,
}

impl TestServer {
    async fn start(customize: impl FnOnce(&mut ServerOptions)) -> Self {
        let td = |f: &str| -> std::path::PathBuf {
            format!("{}/../../testdata/{f}", env!("CARGO_MANIFEST_DIR")).into()
        };
        Self::start_with_files(
            vec![
                td("spacetrack_gp.json"),
                td("sample.3le"),
                td("celestrak_gp.json"),
            ],
            customize,
        )
        .await
    }

    async fn start_with_files(
        paths: Vec<std::path::PathBuf>,
        customize: impl FnOnce(&mut ServerOptions),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let files = FileSource::new(FileConfig { paths });
        let sources = SourceSet::new(vec![Source::File(files)], Cache::new(dir.path()));

        let target: DateTime<Utc> = DateTime::parse_from_rfc3339(AT).unwrap().to_utc();
        let mut opts = ServerOptions {
            tick: Duration::from_millis(100),
            time_offset: target - Utc::now(),
            refresh_minute: Some(10),
            default_filter: Filter {
                max_epoch_age_days: Some(30.0),
                ..Filter::default()
            },
            ..Default::default()
        };
        customize(&mut opts);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(serve_with_listener(listener, opts, sources, async move {
            let _ = rx.await;
        }));
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        // 等待就绪
        for _ in 0..100 {
            if client
                .get(format!("http://{addr}/api/v1/status"))
                .send()
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Self {
            addr,
            client,
            stop: Some(tx),
            handle: Some(handle),
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.client.get(self.url(path)).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        let h = self.handle.take().unwrap();
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("服务未能在 5 秒内关闭")
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn status_reports_catalog_and_sources() {
    let s = TestServer::start(|_| {}).await;
    let (code, v) = s.get("/api/v1/status").await;
    assert_eq!(code, 200);
    assert_eq!(v["objects"], 4);
    assert_eq!(v["generation"], 1);
    assert_eq!(v["sources"][0]["label"], "file");
    assert_eq!(v["sources"][0]["ok"], true);
    assert_eq!(v["schedule"]["refresh_minute"], 10);
    assert!(v["schedule"]["next_refresh"].is_string());
    assert!(v["server_time"].as_str().unwrap().starts_with("2024-01-01T12:3"));

    let html = s
        .client
        .get(s.url("/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("OrbitScope"));
    s.stop().await;
}

#[tokio::test]
async fn list_objects_filters_paging_sorting() {
    let s = TestServer::start(|_| {}).await;
    let (code, v) = s.get("/api/v1/objects").await;
    assert_eq!(code, 200);
    assert_eq!(v["total"], 4);
    let ids: Vec<u64> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["norad_id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, vec![25544, 90001, 90002, 270_000_123]);
    assert!(v["items"][0]["alt_km"].as_f64().unwrap() > 390.0);

    let (_, v) = s.get("/api/v1/objects?types=debris,rocket-body").await;
    assert_eq!(v["total"], 2);

    let (_, v) = s.get("/api/v1/objects?per_page=3&page=2").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["total"], 4);

    let (_, v) = s.get("/api/v1/objects?sort=alt&desc=true&per_page=1").await;
    assert_eq!(v["items"][0]["norad_id"], 90002);

    let (_, v) = s
        .get("/api/v1/objects?alt_min=1200&alt_max=1250&alt_mode=instantaneous")
        .await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["norad_id"], 90002);

    let (_, v) = s.get("/api/v1/objects?q=zarya").await;
    assert_eq!(v["total"], 1);

    // 时间偏移参数:+1d 后默认 30 天龄期仍有效
    let (_, v) = s.get("/api/v1/objects?at=%2B1d").await;
    assert!(v["time"].as_str().unwrap().starts_with("2024-01-02"));

    for bad in [
        "/api/v1/objects?types=debriss",
        "/api/v1/objects?alt_min=900&alt_max=100",
        "/api/v1/objects?sort=nope",
        "/api/v1/objects?at=yesterday",
        "/api/v1/objects?alt_mode=sideways",
    ] {
        let (code, v) = s.get(bad).await;
        assert_eq!(code, 400, "{bad}: {v}");
    }
    s.stop().await;
}

#[tokio::test]
async fn object_detail_track_and_stats() {
    let s = TestServer::start(|_| {}).await;
    let (code, v) = s.get("/api/v1/objects/25544").await;
    assert_eq!(code, 200);
    assert_eq!(v["object"]["name"], "ISS (ZARYA)");
    let alt = v["state"]["geodetic"]["alt_km"].as_f64().unwrap();
    assert!((390.0..450.0).contains(&alt), "{alt}");
    assert_eq!(v["summary"]["object_type"], "PAYLOAD");

    let (code, _) = s.get("/api/v1/objects/12345").await;
    assert_eq!(code, 404);

    let (code, v) = s
        .get("/api/v1/objects/25544/track?step_s=60&after_min=90&before_min=10")
        .await;
    assert_eq!(code, 200);
    assert_eq!(v["points"].as_array().unwrap().len(), 101);
    let (code, _) = s.get("/api/v1/objects/25544/track?step_s=0.1").await;
    assert_eq!(code, 400);

    let (code, v) = s.get("/api/v1/stats?bin_km=100").await;
    assert_eq!(code, 200);
    assert_eq!(v["total"], 4);
    assert_eq!(v["by_type"]["DEBRIS"], 1);
    s.stop().await;
}

#[tokio::test]
async fn default_filter_and_refresh_from_localhost() {
    let s = TestServer::start(|_| {}).await;
    let (_, v) = s.get("/api/v1/filters/default").await;
    assert_eq!(v["alt_max_km"], 1000.0);

    let r = s
        .client
        .put(s.url("/api/v1/filters/default"))
        .json(&json!({"alt_max_km": 500}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (_, v) = s.get("/api/v1/objects").await;
    // ISS(~420 km)与 9 位编号测试物体(560–574 km)中只剩 ISS
    assert_eq!(v["total"], 1);

    let r = s
        .client
        .put(s.url("/api/v1/filters/default"))
        .json(&json!({"typo": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let before = s.get("/api/v1/status").await.1["schedule"]["last_refresh"].clone();
    let r = s.client.post(s.url("/api/v1/refresh")).send().await.unwrap();
    assert_eq!(r.status(), 202);
    // 手动刷新会执行,但数据未变时编目代数不变(客户端无需重新下载元数据)
    let mut after = before.clone();
    for _ in 0..100 {
        after = s.get("/api/v1/status").await.1["schedule"]["last_refresh"].clone();
        if after != before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_ne!(after, before, "刷新应已执行");
    assert_eq!(s.get("/api/v1/status").await.1["generation"], 1);
    s.stop().await;
}

#[tokio::test]
async fn write_endpoints_require_token_when_configured() {
    let s = TestServer::start(|o| o.api_token = Some("test-token-123".into())).await;
    let put = |auth: Option<&str>| {
        let mut req = s
            .client
            .put(s.url("/api/v1/filters/default"))
            .json(&json!({"alt_min_km": 200}));
        if let Some(a) = auth {
            req = req.header("authorization", a);
        }
        req.send()
    };
    assert_eq!(put(None).await.unwrap().status(), 401);
    assert_eq!(put(Some("Bearer wrong-token-12")).await.unwrap().status(), 401);
    assert_eq!(put(Some("Bearer test-token-123")).await.unwrap().status(), 200);
    // 读接口不需要 token
    assert_eq!(s.get("/api/v1/filters/default").await.1["alt_min_km"], 200.0);
    s.stop().await;
}

async fn ws_connect(
    s: &TestServer,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/v1/stream", s.addr))
        .await
        .unwrap();
    ws
}

async fn next_text<S>(ws: &mut S) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("等待消息超时")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

async fn next_binary<S>(ws: &mut S) -> Vec<u8>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("等待帧超时")
            .unwrap()
            .unwrap();
        if let Message::Binary(b) = msg {
            return b.to_vec();
        }
    }
}

#[tokio::test]
async fn websocket_subscribe_and_frames() {
    let s = TestServer::start(|_| {}).await;
    let mut ws = ws_connect(&s).await;

    let hello = next_text(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["frame_format"], "OSF1");

    ws.send(Message::Text(
        json!({"type":"subscribe","filter":{"object_types":["DEBRIS"]},"rate_hz":5})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let cat = next_text(&mut ws).await;
    assert_eq!(cat["type"], "catalog", "{cat}");
    assert_eq!(cat["total"], 1);
    let obj = &cat["objects"][0];
    assert_eq!(obj["id"], 90001);
    let idx = obj["i"].as_u64().unwrap() as u32;

    let f = frame::decode(&next_binary(&mut ws).await).unwrap();
    assert_eq!(f.generation, 1);
    assert_eq!(f.entries.len(), 1);
    assert_eq!(f.entries[0].0, idx);
    let [x, y, z] = f.entries[0].1;
    let r = (x * x + y * y + z * z).sqrt();
    assert!((7100.0..7200.0).contains(&r), "地心距 {r} km");
    assert!(f.time.to_rfc3339().starts_with("2024-01-01T12:3"));

    // 帧随时间推进
    let f2 = frame::decode(&next_binary(&mut ws).await).unwrap();
    assert!(f2.time > f.time);

    // 重新订阅:全部物体
    ws.send(Message::Text(
        json!({"type":"subscribe","filter":{}}).to_string().into(),
    ))
    .await
    .unwrap();
    let cat = next_text(&mut ws).await;
    assert_eq!(cat["total"], 4);

    // ping / 错误消息
    ws.send(Message::Text(json!({"type":"ping"}).to_string().into()))
        .await
        .unwrap();
    assert_eq!(next_text(&mut ws).await["type"], "pong");
    ws.send(Message::Text(
        json!({"type":"subscribe","filter":{"alt_min_km":5000}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    assert_eq!(next_text(&mut ws).await["type"], "error");
    ws.send(Message::Text("garbage".into())).await.unwrap();
    assert_eq!(next_text(&mut ws).await["type"], "error");

    let (_, st) = s.get("/api/v1/status").await;
    assert_eq!(st["ws_clients"], 1);
    ws.close(None).await.unwrap();
    drop(ws);
    s.stop().await;
}

#[tokio::test]
async fn websocket_connection_limit() {
    let s = TestServer::start(|o| o.max_ws_clients = 1).await;
    let mut first = ws_connect(&s).await;
    assert_eq!(next_text(&mut first).await["type"], "hello");
    let second = tokio_tungstenite::connect_async(format!("ws://{}/api/v1/stream", s.addr)).await;
    assert!(second.is_err(), "超过上限的连接应被拒绝");
    drop(first);
    // 第一个连接关闭后,计数恢复
    let mut ok = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if tokio_tungstenite::connect_async(format!("ws://{}/api/v1/stream", s.addr))
            .await
            .is_ok()
        {
            ok = true;
            break;
        }
    }
    assert!(ok);
    s.stop().await;
}

#[tokio::test]
async fn shutdown_closes_open_websockets() {
    let s = TestServer::start(|_| {}).await;
    let mut ws = ws_connect(&s).await;
    next_text(&mut ws).await;
    // 有打开的 WS 连接时也必须能在超时内关闭
    s.stop().await;
}

#[tokio::test]
async fn web_frontend_and_client_config() {
    let s = TestServer::start(|_| {}).await;
    let r = s.client.get(s.url("/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(r.text().await.unwrap().contains("js/main.js"));
    for (path, ctype) in [
        ("/js/main.js", "text/javascript"),
        ("/js/stream.js", "text/javascript"),
        ("/js/i18n.js", "text/javascript"),
        ("/i18n/zh-CN.json", "application/json"),
        ("/i18n/en.json", "application/json"),
        ("/i18n/zh-HK.json", "application/json"),
        ("/css/app.css", "text/css"),
    ] {
        let r = s.client.get(s.url(path)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert!(
            r.headers()["content-type"].to_str().unwrap().starts_with(ctype),
            "{path}"
        );
    }
    assert_eq!(
        s.client
            .get(s.url("/no-such-file.js"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let (code, v) = s.get("/api/v1/no-such-endpoint").await;
    assert_eq!(code, 404);
    assert!(v["error"].is_string());

    let (_, cfg) = s.get("/api/v1/client-config").await;
    assert_eq!(cfg["cesium_base_url"], orbit_server::DEFAULT_CESIUM_URL);
    assert_eq!(cfg["tick_ms"], 100);
    // 未配置本地目录时不提供 /cesium/
    assert_eq!(
        s.client
            .get(s.url("/cesium/Cesium.js"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    s.stop().await;
}

#[tokio::test]
async fn local_cesium_dir_is_served() {
    let cesium = tempfile::tempdir().unwrap();
    std::fs::write(cesium.path().join("Cesium.js"), "/* fake cesium */").unwrap();
    std::fs::create_dir_all(cesium.path().join("Widgets")).unwrap();
    std::fs::write(cesium.path().join("Widgets/widgets.css"), "body{}").unwrap();
    let dir = cesium.path().to_path_buf();
    let s = TestServer::start(move |o| o.cesium_dir = Some(dir)).await;
    let (_, cfg) = s.get("/api/v1/client-config").await;
    assert_eq!(cfg["cesium_base_url"], "/cesium/");
    let r = s.client.get(s.url("/cesium/Cesium.js")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "/* fake cesium */");
    assert_eq!(
        s.client
            .get(s.url("/cesium/Widgets/widgets.css"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        s.client
            .get(s.url("/cesium/../Cargo.toml"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    s.stop().await;
}

// ---------------------------------------------------------------- 历史归档与设置

/// 复制 testdata 文件到 `dir`,修改时间(获取时间)设为 2024-01-01T12:10:00Z。
fn fetched_copy(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let src = format!("{}/../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    let dst = dir.join(name);
    std::fs::copy(src, &dst).unwrap();
    let t: std::time::SystemTime = DateTime::parse_from_rfc3339("2024-01-01T12:10:00Z")
        .unwrap()
        .to_utc()
        .into();
    std::fs::File::options()
        .write(true)
        .open(&dst)
        .unwrap()
        .set_modified(t)
        .unwrap();
    dst
}

fn archive_in(dir: &std::path::Path) -> std::sync::Arc<orbit_archive::Archive> {
    let cfg = orbit_archive::ArchiveConfig {
        dir: Some(dir.to_path_buf()),
        ..Default::default()
    };
    std::sync::Arc::new(orbit_archive::Archive::open(&cfg).unwrap())
}

async fn wait_for(s: &TestServer, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..150 {
        let v = s.get("/api/v1/status").await.1;
        if pred(&v) {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等待状态超时: {}", s.get("/api/v1/status").await.1);
}

async fn put_settings(s: &TestServer, body: Value) -> (u16, Value) {
    let r = s
        .client
        .put(s.url("/api/v1/settings"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let code = r.status().as_u16();
    (code, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn history_known_at_and_runtime_settings() {
    let data = tempfile::tempdir().unwrap();
    let f = fetched_copy(data.path(), "spacetrack_gp.json");
    let arch = archive_in(&data.path().join("archive"));
    let settings_path = data.path().join("archive/ui-settings.json");
    let sp = settings_path.clone();
    let s = TestServer::start_with_files(vec![f], move |o| {
        o.archive = Some(arch);
        o.settings_path = Some(sp);
    })
    .await;

    // 服务端时刻 12:30,文件 12:10 获取 → known-at 可见(ISS 与 90001;90004 已再入)
    let st = s.get("/api/v1/status").await.1;
    assert_eq!(st["objects"], 2, "{st}");
    assert_eq!(st["provenance"]["policy"], "known-at");
    assert_eq!(st["provenance"]["first_seen_max"], "2024-01-01T12:10:00Z");
    let (_, set) = s.get("/api/v1/settings").await;
    assert_eq!(set["time_policy"], "known-at");
    assert_eq!(set["time_mode"], "fixed");
    assert_eq!(set["history_enabled"], true);
    assert_eq!(set["policies"].as_array().unwrap().len(), 3);
    let (_, list) = s.get("/api/v1/objects").await;
    assert_eq!(list["items"][0]["fetched_at"], "2024-01-01T12:10:00Z");

    // 切换到获取时间之前的时刻:known-at 下没有数据
    let (code, v) = put_settings(&s, json!({"time": "2024-01-01T12:05:00Z"})).await;
    assert_eq!(code, 200, "{v}");
    let st = wait_for(&s, |v| v["objects"] == 0).await;
    assert_eq!(st["generation"], 2);

    // 同一时刻切换为 nearest-epoch:数据回来
    put_settings(&s, json!({"time_policy": "nearest-epoch"})).await;
    let st = wait_for(&s, |v| v["objects"] == 2).await;
    assert_eq!(st["provenance"]["policy"], "nearest-epoch");

    // 设置已保存到磁盘(时间不保存)
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({"time_policy": "nearest-epoch", "collect_mode": "serve"})
    );

    // 切换采集策略为 daemon:不再安排联网采集
    let (_, v) = put_settings(&s, json!({"collect_mode": "daemon"})).await;
    assert_eq!(v["collect_mode"], "daemon");
    let st = wait_for(&s, |v| v["schedule"]["next_refresh"].is_null()).await;
    assert!(st["schedule"]["next_reload"].is_string());

    // 回到实时
    let (_, v) = put_settings(&s, json!({"time": "live"})).await;
    assert_eq!(v["time_mode"], "live");

    // 非法值
    for bad in [
        json!({"time_policy": "x"}),
        json!({"collect_mode": "cron"}),
        json!({"time": "yesterday"}),
        json!({"typo": 1}),
    ] {
        let (code, _) = put_settings(&s, bad.clone()).await;
        assert!(code == 400 || code == 422, "{bad}: {code}");
    }
    s.stop().await;
}

#[tokio::test]
async fn daemon_mode_server_never_collects() {
    let data = tempfile::tempdir().unwrap();
    let f = fetched_copy(data.path(), "spacetrack_gp.json");
    let arch = archive_in(&data.path().join("archive"));
    let a2 = arch.clone();
    let s = TestServer::start_with_files(vec![f], move |o| {
        o.archive = Some(arch);
        o.collect.mode = orbit_archive::CollectMode::Daemon;
    })
    .await;
    // 历史库为空,服务端不会自行采集 → 没有数据
    let st = s.get("/api/v1/status").await.1;
    assert_eq!(st["objects"], 0);
    assert!(st["schedule"]["next_refresh"].is_null());
    assert_eq!(
        a2.history().unwrap().usage().unwrap().elsets,
        0,
        "daemon 模式下 serve 不写入历史"
    );

    // 模拟独立采集进程写入历史后,服务端重新加载即可看到
    let src = FileSource::new(FileConfig {
        paths: vec![data.path().join("spacetrack_gp.json")],
    });
    let set = SourceSet::new(vec![Source::File(src)], Cache::new(data.path().join("c2")));
    let result = set.fetch_all(orbit_sources::FetchMode::Normal, Utc::now()).await;
    a2.record(&result, set.cache(), Utc::now()).unwrap();
    let r = s.client.post(s.url("/api/v1/refresh")).send().await.unwrap();
    assert_eq!(r.status(), 202);
    // 刷新请求在 daemon 模式下只重新加载
    wait_for(&s, |v| v["objects"] == 2).await;
    s.stop().await;
}
