//! CLI 端到端测试:运行编译出的二进制,只使用本地文件数据源(不联网)。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const AT: &str = "2024-01-01T12:30:00Z";

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// 只启用本地文件的配置。
fn write_config(dir: &Path) -> PathBuf {
    let files = ["spacetrack_gp.json", "sample.3le", "celestrak_gp.json"]
        .map(|f| format!("{:?}", testdata(f).display().to_string()))
        .join(", ");
    let cfg = format!(
        "[filter]\nmax_epoch_age_days = 30\n\n[sources]\ncache_dir = {:?}\n\n[sources.celestrak]\nenabled = false\n\n[sources.files]\npaths = [{files}]\n",
        dir.join("cache").display().to_string()
    );
    let p = dir.join("test.toml");
    std::fs::write(&p, cfg).unwrap();
    p
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orbitscope"))
        .args(args)
        .current_dir(dir)
        .env_remove("ORBITSCOPE_CONFIG")
        .env_remove("ORBITSCOPE_SPACETRACK_USER")
        .env_remove("ORBITSCOPE_SPACETRACK_PASSWORD")
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "命令失败: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn list_json_default_filter() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let out = ok(
        dir.path(),
        &[
            "-c",
            cfg.to_str().unwrap(),
            "--quiet",
            "list",
            "--at",
            AT,
            "-f",
            "json",
        ],
    );
    let rows: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
    let ids: Vec<u64> = rows.iter().map(|r| r["norad_id"].as_u64().unwrap()).collect();
    assert_eq!(ids, vec![25544, 90001, 90002, 270_000_123]);
    let iss = &rows[0];
    assert_eq!(iss["object_type"], "PAYLOAD");
    let alt = iss["alt_km"].as_f64().unwrap();
    assert!((390.0..450.0).contains(&alt), "{alt}");
}

#[test]
fn list_filters_and_sorting() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let c = cfg.to_str().unwrap();

    let out = ok(
        dir.path(),
        &[
            "-c", c, "--quiet", "list", "--at", AT, "--alt", "400..450", "-f", "csv",
        ],
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("norad_id,name,"));
    assert!(lines[1].starts_with("25544,"));

    let out = ok(
        dir.path(),
        &[
            "-c", c, "--quiet", "list", "--at", AT, "--type", "debris", "-f", "csv",
        ],
    );
    assert_eq!(out.lines().count(), 2);
    assert!(out.contains("90001,TESTSAT DEB"));

    let out = ok(
        dir.path(),
        &[
            "-c", c, "--quiet", "list", "--at", AT, "--sort", "alt", "--desc", "-n", "1", "-f", "json",
        ],
    );
    let rows: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["norad_id"], 90002, "最高的是偏心轨道火箭体");

    // 尺寸:≥1 m 且排除未知 → 只剩 ISS(LARGE)
    let out = ok(
        dir.path(),
        &[
            "-c",
            c,
            "--quiet",
            "list",
            "--at",
            AT,
            "--min-size",
            "100",
            "--unknown-size",
            "exclude",
            "-f",
            "csv",
        ],
    );
    assert_eq!(out.lines().count(), 2);
    assert!(out.contains("25544,"));

    // 瞬时高度模式
    let out = ok(
        dir.path(),
        &[
            "-c",
            c,
            "--quiet",
            "list",
            "--at",
            AT,
            "--alt",
            "1200..1250",
            "--alt-mode",
            "instantaneous",
            "-f",
            "csv",
        ],
    );
    assert!(out.lines().nth(1).unwrap().starts_with("90002,"), "{out}");
}

#[test]
fn table_output_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let out = run(dir.path(), &["-c", cfg.to_str().unwrap(), "list", "--at", AT]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stdout.contains("ISS (ZARYA)"));
    assert!(stdout.contains("符合条件 4 个"));
    assert!(stderr.contains("[file]"), "未输出数据源状态: {stderr}");
}

#[test]
fn show_with_track() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let c = cfg.to_str().unwrap();
    let out = ok(
        dir.path(),
        &[
            "-c", c, "--quiet", "show", "25544", "--at", AT, "--track", "90m", "--step", "10m", "-f", "json",
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["object"]["norad_id"], 25544);
    assert_eq!(v["track"].as_array().unwrap().len(), 10);
    assert!(v["state"]["geodetic"]["alt_km"].as_f64().unwrap() > 390.0);

    let table = ok(dir.path(), &["-c", c, "--quiet", "show", "25544", "--at", AT]);
    assert!(table.contains("RCS 分档"));

    let missing = run(dir.path(), &["-c", c, "--quiet", "show", "12345", "--at", AT]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("没有 NORAD 12345"));
}

#[test]
fn export_to_file_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let c = cfg.to_str().unwrap();
    let out_file = dir.path().join("out.csv");
    ok(
        dir.path(),
        &[
            "-c",
            c,
            "--quiet",
            "export",
            "--at",
            AT,
            "-o",
            out_file.to_str().unwrap(),
        ],
    );
    let csv = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(csv.lines().count(), 5);

    let s = ok(
        dir.path(),
        &["-c", c, "--quiet", "stats", "--at", AT, "-f", "json"],
    );
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["total"], 4);
    assert_eq!(v["by_type"]["DEBRIS"], 1);

    let bad = run(dir.path(), &["-c", c, "export", "-f", "table"]);
    assert!(!bad.status.success());
}

#[test]
fn relative_time_and_epoch_age() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let c = cfg.to_str().unwrap();
    // 现在(2026 年)距样本根数历元 > 30 天 → 默认龄期过滤后为空
    let out = ok(dir.path(), &["-c", c, "--quiet", "list", "-f", "json"]);
    assert_eq!(out.trim(), "[]");
    // --any-age 取消龄期限制
    let out = ok(
        dir.path(),
        &[
            "-c",
            c,
            "--quiet",
            "list",
            "--any-age",
            "--at",
            "-1h",
            "-f",
            "json",
        ],
    );
    assert!(
        !serde_json::from_str::<Vec<serde_json::Value>>(&out)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn invalid_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let c = cfg.to_str().unwrap();
    for args in [
        vec!["-c", c, "list", "--alt", "600..400"],
        vec!["-c", c, "list", "--at", "yesterday"],
        vec!["-c", c, "list", "--alt", "400..600", "--alt-min", "1"],
        vec!["-c", "/nonexistent/x.toml", "list"],
    ] {
        let out = run(dir.path(), &args);
        assert!(!out.status.success(), "应失败: {args:?}");
    }
}

#[test]
fn offline_without_cache_fails_without_network() {
    let dir = tempfile::tempdir().unwrap();
    // 启用 CelesTrak 但离线:应立即失败且提示先 fetch(base_url 指向不可达地址,确保即使联网也不会成功)
    let cfg = format!(
        "[sources]\ncache_dir = {:?}\n[sources.celestrak]\nbase_url = \"http://127.0.0.1:9\"\n",
        dir.path().join("empty-cache").display().to_string()
    );
    let p = dir.path().join("c.toml");
    std::fs::write(&p, cfg).unwrap();
    let out = run(dir.path(), &["-c", p.to_str().unwrap(), "--offline", "list"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("orbitscope fetch"), "{err}");
}

#[test]
fn config_init_and_show() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub/orbitscope.toml");
    ok(dir.path(), &["config", "init", "--path", p.to_str().unwrap()]);
    assert!(
        std::fs::read_to_string(&p)
            .unwrap()
            .contains("[sources.spacetrack]")
    );
    // 不加 --force 不覆盖
    assert!(
        !run(dir.path(), &["config", "init", "--path", p.to_str().unwrap()])
            .status
            .success()
    );

    // 当前目录下的 orbitscope.toml 会被自动发现
    std::fs::write(
        dir.path().join("orbitscope.toml"),
        "[filter]\nalt_max_km = 555.0\n",
    )
    .unwrap();
    let out = run(dir.path(), &["config", "show"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("alt_max_km = 555.0"));
}

#[test]
fn spacetrack_enabled_without_credentials_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.toml");
    std::fs::write(
        &p,
        format!(
            "[sources]\ncache_dir = {:?}\n[sources.spacetrack]\nenabled = true\n",
            dir.path().display().to_string()
        ),
    )
    .unwrap();
    let out = run(dir.path(), &["-c", p.to_str().unwrap(), "fetch"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ORBITSCOPE_SPACETRACK_USER"));
}
