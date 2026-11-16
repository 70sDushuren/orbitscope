//! 数据源集成测试。全部使用 wiremock 本地模拟服务器,绝不访问真实 Space-Track / CelesTrak。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{Duration, Utc};
use orbit_core::{DataSource, ObjectType, RcsInfo};
use orbit_sources::celestrak::{CelesTrakConfig, CelesTrakSource};
use orbit_sources::file::FileConfig;
use orbit_sources::http::RetryPolicy;
use orbit_sources::spacetrack::{Credentials, SpaceTrackConfig, SpaceTrackSource};
use orbit_sources::{Cache, CacheMeta, FetchMode, Origin, Source, SourceError, SourceSet, SourcesConfig};
use wiremock::matchers::{
    body_string_contains, header, header_exists, method, path, path_regex, query_param,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn testdata(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../../testdata/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

const SATCAT: &str = "OBJECT_NAME,OBJECT_ID,NORAD_CAT_ID,OBJECT_TYPE,OPS_STATUS_CODE,OWNER,LAUNCH_DATE,LAUNCH_SITE,DECAY_DATE,PERIOD,INCLINATION,APOGEE,PERIGEE,RCS,DATA_STATUS_CODE,ORBIT_CENTER,ORBIT_TYPE\n\
ISS (ZARYA),1998-067A,25544,PAY,+,ISS,1998-11-20,TYMSC,,92.9,51.64,422,413,399.0524,,EA,ORB\n";

fn celestrak_cfg(base: &str, groups: &[&str], satcat: bool) -> CelesTrakConfig {
    CelesTrakConfig {
        base_url: base.to_string(),
        groups: groups.iter().map(|s| s.to_string()).collect(),
        satcat,
        ..Default::default()
    }
}

fn celestrak(base: &str, groups: &[&str], satcat: bool, cache: &Cache) -> CelesTrakSource {
    CelesTrakSource::new(celestrak_cfg(base, groups, satcat), cache.clone())
        .unwrap()
        .with_retry(RetryPolicy::none())
}

async fn mount_gp(server: &MockServer, group: &str, body: String, times: u64) {
    Mock::given(method("GET"))
        .and(path("/NORAD/elements/gp.php"))
        .and(query_param("GROUP", group))
        .and(query_param("FORMAT", "json"))
        .and(header("user-agent", orbit_sources::USER_AGENT))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("etag", "\"v1\""),
        )
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn celestrak_fetch_then_cache() {
    let server = MockServer::start().await;
    mount_gp(&server, "active", testdata("celestrak_gp.json"), 1).await;
    Mock::given(method("GET"))
        .and(path("/pub/satcat.csv"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SATCAT))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let src = celestrak(&server.uri(), &["active"], true, &cache);
    let now = Utc::now();

    let b1 = src.fetch(FetchMode::Normal, now).await.unwrap();
    assert_eq!(b1.records.len(), 2);
    assert_eq!(b1.record_errors, 1, "缺 EPOCH 的记录被跳过");
    assert_eq!(b1.extra_meta.len(), 1);
    assert!(b1.resources.iter().all(|r| r.origin == Origin::Network));

    // 30 分钟后再取:在刷新间隔内,不应联网(expect(1) 在 server drop 时校验)
    let b2 = src
        .fetch(FetchMode::Normal, now + Duration::minutes(30))
        .await
        .unwrap();
    assert_eq!(b2.records.len(), 2);
    assert!(
        b2.resources
            .iter()
            .all(|r| matches!(r.origin, Origin::Cache { age_secs: 1800 }))
    );
}

#[tokio::test]
async fn celestrak_refresh_interval_has_floor() {
    let server = MockServer::start().await;
    mount_gp(&server, "active", testdata("celestrak_gp.json"), 1).await;
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    // 配置 1 分钟刷新也会被提升到 60 分钟下限
    let cfg = CelesTrakConfig {
        refresh_minutes: 1,
        ..celestrak_cfg(&server.uri(), &["active"], false)
    };
    let src = CelesTrakSource::new(cfg, cache)
        .unwrap()
        .with_retry(RetryPolicy::none());
    let now = Utc::now();
    src.fetch(FetchMode::Normal, now).await.unwrap();
    let b = src
        .fetch(FetchMode::Normal, now + Duration::minutes(10))
        .await
        .unwrap();
    assert!(matches!(b.resources[0].origin, Origin::Cache { .. }));
}

#[tokio::test]
async fn celestrak_conditional_get_304() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/NORAD/elements/gp.php"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let url = format!("{}/NORAD/elements/gp.php?GROUP=active&FORMAT=json", server.uri());
    let old = Utc::now() - Duration::hours(5);
    let meta = CacheMeta {
        url,
        fetched_at: old,
        etag: Some("\"v1\"".into()),
        last_modified: None,
        bytes: 0,
    };
    cache
        .write("celestrak/gp-active", &meta, &testdata("celestrak_gp.json"))
        .await
        .unwrap();

    let src = celestrak(&server.uri(), &["active"], false, &cache);
    let now = Utc::now();
    let b = src.fetch(FetchMode::Normal, now).await.unwrap();
    assert_eq!(b.resources[0].origin, Origin::NotModified);
    assert_eq!(b.records.len(), 2);
    // 304 后 fetched_at 更新
    let m = cache.read_meta("celestrak/gp-active").await.unwrap().unwrap();
    assert_eq!(m.fetched_at, now);
}

#[tokio::test]
async fn celestrak_falls_back_to_stale_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let url = format!("{}/NORAD/elements/gp.php?GROUP=active&FORMAT=json", server.uri());
    let meta = CacheMeta {
        url,
        fetched_at: Utc::now() - Duration::days(2),
        etag: None,
        last_modified: None,
        bytes: 0,
    };
    cache
        .write("celestrak/gp-active", &meta, &testdata("celestrak_gp.json"))
        .await
        .unwrap();

    let src = celestrak(&server.uri(), &["active"], false, &cache);
    let b = src.fetch(FetchMode::Normal, Utc::now()).await.unwrap();
    assert!(matches!(&b.resources[0].origin, Origin::StaleCache { error, .. } if error.contains("500")));
    assert_eq!(b.records.len(), 2);
}

#[tokio::test]
async fn celestrak_error_without_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let src = celestrak(&server.uri(), &["active"], false, &Cache::new(dir.path()));
    let err = src.fetch(FetchMode::Normal, Utc::now()).await.unwrap_err();
    assert!(matches!(err, SourceError::Status { status: 503, .. }), "{err}");
}

#[tokio::test]
async fn celestrak_retries_transient_errors_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_gp(&server, "active", testdata("celestrak_gp.json"), 1).await;
    let dir = tempfile::tempdir().unwrap();
    let src = CelesTrakSource::new(
        celestrak_cfg(&server.uri(), &["active"], false),
        Cache::new(dir.path()),
    )
    .unwrap()
    .with_retry(RetryPolicy {
        max_attempts: 2,
        base_delay: std::time::Duration::ZERO,
        ..Default::default()
    });
    let b = src.fetch(FetchMode::Normal, Utc::now()).await.unwrap();
    assert_eq!(b.records.len(), 2);
}

#[tokio::test]
async fn celestrak_bad_group_is_a_note() {
    let server = MockServer::start().await;
    mount_gp(&server, "active", testdata("celestrak_gp.json"), 1).await;
    mount_gp(&server, "nonexistent", "No GP data found".into(), 1).await;
    let dir = tempfile::tempdir().unwrap();
    let src = celestrak(
        &server.uri(),
        &["active", "nonexistent"],
        false,
        &Cache::new(dir.path()),
    );
    let b = src.fetch(FetchMode::Normal, Utc::now()).await.unwrap();
    assert_eq!(b.records.len(), 2);
    assert!(
        b.notes
            .iter()
            .any(|n| n.contains("nonexistent") && n.contains("No GP data")),
        "{:?}",
        b.notes
    );
}

#[tokio::test]
async fn cache_only_never_touches_network() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let src = celestrak(&server.uri(), &["active"], false, &Cache::new(dir.path()));
    let err = src.fetch(FetchMode::CacheOnly, Utc::now()).await.unwrap_err();
    assert!(matches!(err, SourceError::NoCache(_)));
}

#[test]
fn invalid_group_name_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = celestrak_cfg("http://localhost", &["active&FORMAT=tle"], false);
    assert!(matches!(
        CelesTrakSource::new(cfg, Cache::new(dir.path())),
        Err(SourceError::Config(_))
    ));
}

// ---------------- Space-Track ----------------

fn spacetrack(base: &str, cache: &Cache) -> SpaceTrackSource {
    let cfg = SpaceTrackConfig {
        enabled: true,
        base_url: base.to_string(),
        ..Default::default()
    };
    SpaceTrackSource::new(
        cfg,
        Credentials::new("tester@example.com", "s3cret-pw"),
        cache.clone(),
    )
    .unwrap()
    .with_retry(RetryPolicy::none())
}

async fn mount_login_ok(server: &MockServer, times: u64) {
    Mock::given(method("POST"))
        .and(path("/ajaxauth/login"))
        .and(body_string_contains("identity=tester%40example.com"))
        .and(body_string_contains("password=s3cret-pw"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("\"\"")
                .insert_header("set-cookie", "chocolatechip=abc123; Path=/"),
        )
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn spacetrack_login_query_and_cache() {
    let server = MockServer::start().await;
    mount_login_ok(&server, 1).await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/basicspacedata/query/class/gp/decay_date/null-val/epoch/",
        ))
        .and(header("cookie", "chocolatechip=abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_string(testdata("spacetrack_gp.json")))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let src = spacetrack(&server.uri(), &cache);
    let now = Utc::now();
    let b = src.fetch(FetchMode::Normal, now).await.unwrap();
    assert_eq!(b.records.len(), 3);
    assert_eq!(b.source, DataSource::SpaceTrack);
    assert_eq!(b.resources[0].origin, Origin::Network);

    // 59 分钟后:仍在 1 小时规定内,必须用缓存
    let b2 = src
        .fetch(FetchMode::Normal, now + Duration::minutes(59))
        .await
        .unwrap();
    assert!(matches!(b2.resources[0].origin, Origin::Cache { .. }));

    // 新实例(模拟程序重启)也必须用磁盘缓存
    let src2 = spacetrack(&server.uri(), &cache);
    let b3 = src2
        .fetch(FetchMode::Normal, now + Duration::minutes(30))
        .await
        .unwrap();
    assert!(matches!(b3.resources[0].origin, Origin::Cache { .. }));
}

#[tokio::test]
async fn spacetrack_login_failure_hides_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ajaxauth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"Login":"Failed"}"#))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let err = spacetrack(&server.uri(), &Cache::new(dir.path()))
        .fetch(FetchMode::Normal, Utc::now())
        .await
        .unwrap_err();
    let msg = format!("{err} {err:?}");
    assert!(matches!(err, SourceError::Login(_)));
    assert!(!msg.contains("s3cret-pw"), "错误信息泄露了密码: {msg}");
}

#[tokio::test]
async fn spacetrack_relogin_on_401() {
    let server = MockServer::start().await;
    mount_login_ok(&server, 2).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/basicspacedata/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(testdata("spacetrack_gp.json")))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let b = spacetrack(&server.uri(), &Cache::new(dir.path()))
        .fetch(FetchMode::Normal, Utc::now())
        .await
        .unwrap();
    assert_eq!(b.records.len(), 3);
}

#[tokio::test]
async fn spacetrack_requests_carry_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("user-agent", orbit_sources::USER_AGENT))
        .respond_with(ResponseTemplate::new(200).set_body_string("\"\""))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(header_exists("user-agent"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let b = spacetrack(&server.uri(), &Cache::new(dir.path()))
        .fetch(FetchMode::Normal, Utc::now())
        .await
        .unwrap();
    assert!(b.records.is_empty());
}

// ---------------- SourceSet ----------------

#[test]
fn spacetrack_enabled_requires_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = SourcesConfig {
        cache_dir: Some(dir.path().to_path_buf()),
        spacetrack: SpaceTrackConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = SourceSet::from_config(&cfg, None).unwrap_err();
    assert!(matches!(err, SourceError::MissingCredentials(_)));
}

#[test]
fn config_from_toml_like_json() {
    let cfg: SourcesConfig =
        serde_json::from_str(r#"{"celestrak":{"groups":["active"]},"spacetrack":{"enabled":true}}"#).unwrap();
    assert_eq!(cfg.celestrak.groups, vec!["active"]);
    assert!(cfg.celestrak.enabled);
    assert_eq!(cfg.spacetrack.refresh_minutes, 60);
    assert!(serde_json::from_str::<SourcesConfig>(r#"{"celestrak":{"typo":1}}"#).is_err());
}

#[tokio::test]
async fn source_set_merges_and_builds_catalog() {
    let server = MockServer::start().await;
    mount_gp(&server, "active", testdata("celestrak_gp.json"), 1).await;
    Mock::given(method("GET"))
        .and(path("/pub/satcat.csv"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SATCAT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg = SourcesConfig {
        cache_dir: Some(dir.path().join("cache")),
        celestrak: celestrak_cfg(&server.uri(), &["active"], true),
        files: FileConfig {
            paths: vec![
                format!("{}/../../testdata/sample.3le", env!("CARGO_MANIFEST_DIR")).into(),
                dir.path().join("missing.tle"),
            ],
        },
        ..Default::default()
    };
    let set = SourceSet::from_config(&cfg, None).unwrap();
    assert_eq!(
        set.sources().iter().map(Source::label).collect::<Vec<_>>(),
        ["celestrak", "file"]
    );

    let now = chrono::DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
        .unwrap()
        .to_utc();
    let result = set.fetch_all(FetchMode::Normal, now).await;
    assert!(result.any_ok());
    let file_status = result.statuses.iter().find(|s| s.label == "file").unwrap();
    assert!(file_status.ok);
    assert!(file_status.notes.iter().any(|n| n.contains("missing.tle")));

    let (cat, report) = result.build_catalog(now);
    // celestrak: 25544 + 9 位编号;file: 25544 + 90001 + 90002
    assert_eq!(report.duplicates, 1);
    assert_eq!(cat.len(), 4);
    let iss = cat.by_norad(25544).unwrap();
    assert_eq!(iss.meta.object_type, ObjectType::Payload, "SATCAT 类型应合并进来");
    assert_eq!(iss.meta.country.as_deref(), Some("ISS"));
    assert!(matches!(iss.meta.rcs, RcsInfo::Numeric { .. }));
    assert_eq!(cat.by_norad(90001).unwrap().meta.object_type, ObjectType::Debris);
}
