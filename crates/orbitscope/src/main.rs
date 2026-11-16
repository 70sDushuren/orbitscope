//! OrbitScope 命令行入口。

mod args;
mod commands;
mod config;
mod output;

use anyhow::Result;
use args::FilterArgs;
use clap::{Parser, Subcommand};
use commands::{Ctx, ListOpts, ServeOpts, ShowOpts};
use output::{Format, SortKey};
use std::path::PathBuf;

/// 准实时近地轨道空间物体监视程序
#[derive(Debug, Parser)]
#[command(name = "orbitscope", version, about, long_about = None)]
struct Cli {
    /// 配置文件路径(默认查找 ./orbitscope.toml 与用户配置目录)
    #[arg(short, long, global = true, env = "ORBITSCOPE_CONFIG", value_name = "FILE")]
    config: Option<PathBuf>,

    /// 只使用本地缓存,不联网
    #[arg(long, global = true)]
    offline: bool,

    /// 不输出数据源状态(出现问题时仍会输出)
    #[arg(long, global = true)]
    quiet: bool,

    /// 日志详细程度(-v、-vv);也可用 RUST_LOG
    /// 指定时刻时的时刻策略:known-at(该时刻已获取到的数据)、nearest-epoch(历元最接近)、latest(最新);
    /// 默认取配置 [archive] time_policy
    #[arg(long, global = true, value_name = "POLICY")]
    policy: Option<String>,

    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 从数据源获取最新数据并写入缓存(遵守各数据源的刷新间隔)
    Fetch,

    /// 按条件列出物体及其在指定时刻的位置
    List {
        #[command(flatten)]
        filter: FilterArgs,
        /// 时刻:now、RFC3339,或相对偏移如 +90m、-2h
        #[arg(long, default_value = "now", allow_hyphen_values = true)]
        at: String,
        /// 排序字段
        #[arg(long, value_enum, default_value = "id")]
        sort: SortKey,
        /// 降序
        #[arg(long)]
        desc: bool,
        /// 最多显示条数(0 表示不限)
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        #[arg(short, long, value_enum, default_value = "table")]
        format: Format,
    },

    /// 显示单个物体详情、当前位置与地面轨迹
    Show {
        /// NORAD 编号
        norad_id: u32,
        #[arg(long, default_value = "now", allow_hyphen_values = true)]
        at: String,
        /// 计算从该时刻起的地面轨迹,时长如 90m、2h
        #[arg(long, value_name = "DURATION")]
        track: Option<String>,
        /// 轨迹步长
        #[arg(long, default_value = "1m")]
        step: String,
        #[arg(short, long, value_enum, default_value = "table")]
        format: Format,
    },

    /// 导出过滤结果(含指定时刻位置)为 CSV 或 JSON
    Export {
        #[command(flatten)]
        filter: FilterArgs,
        #[arg(long, default_value = "now", allow_hyphen_values = true)]
        at: String,
        #[arg(short, long, value_enum, default_value = "csv")]
        format: Format,
        /// 输出文件(默认标准输出)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "id")]
        sort: SortKey,
    },

    /// 统计:按类型、国家计数与高度分布
    Stats {
        #[command(flatten)]
        filter: FilterArgs,
        #[arg(long, default_value = "now", allow_hyphen_values = true)]
        at: String,
        /// 高度直方图分箱宽度(km)
        #[arg(long, default_value_t = 50.0)]
        bin: f64,
        #[arg(short, long, value_enum, default_value = "table")]
        format: Format,
    },

    /// 启动 Web 服务(REST API + WebSocket 实时推送)
    Serve {
        /// 监听地址,覆盖配置 server.bind,如 127.0.0.1:8080
        #[arg(long)]
        bind: Option<String>,
        /// 模拟时间:服务端时钟从该时刻开始走(RFC3339 或相对偏移),用于回放或演示
        #[arg(long, allow_hyphen_values = true)]
        sim_time: Option<String>,
    },

    /// 常驻采集:不启动 Web 服务,定期获取数据并写入历史归档(采集策略 daemon)
    Collect {
        /// 只采集一次后退出(等同 fetch,适合系统定时任务)
        #[arg(long)]
        once: bool,
    },

    /// 历史归档:占用状态、采集记录、物体历史根数、完整快照、手动清理
    History {
        #[command(subcommand)]
        action: HistoryAction,
        #[arg(short, long, value_enum, default_value = "table", global = true)]
        format: Format,
    },

    /// 配置文件管理
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
enum HistoryAction {
    /// 归档目录、历史库与快照的占用及上限
    Status,
    /// 最近的采集记录
    Fetches {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 某物体的历史根数
    Object {
        norad_id: u32,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 最近的完整快照
    Snapshots {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// 按保留设置立即清理
    Prune,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// 显示生效的配置
    Show,
    /// 生成带注释的示例配置文件(默认写到用户配置目录)
    Init {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info,orbit_sources=info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    // `config init` 不需要加载已有配置
    if let Command::Config {
        action: ConfigAction::Init { path, force },
    } = cli.command
    {
        return commands::config_init(path, force);
    }

    let loaded = config::load(cli.config.as_deref())?;
    let ctx = Ctx {
        cfg: loaded.config,
        config_path: loaded.path,
        offline: cli.offline,
        quiet: cli.quiet,
        policy: cli
            .policy
            .as_deref()
            .map(str::parse::<orbit_archive::TimePolicy>)
            .transpose()
            .map_err(|e| anyhow::anyhow!(e))?,
    };

    match cli.command {
        Command::Fetch => commands::fetch(&ctx).await,
        Command::Collect { once } => commands::collect(&ctx, once).await,
        Command::History { action, format } => {
            let cmd = match action {
                HistoryAction::Status => commands::HistoryCmd::Status,
                HistoryAction::Fetches { limit } => commands::HistoryCmd::Fetches(limit),
                HistoryAction::Object { norad_id, limit } => commands::HistoryCmd::Object(norad_id, limit),
                HistoryAction::Snapshots { limit } => commands::HistoryCmd::Snapshots(limit),
                HistoryAction::Prune => commands::HistoryCmd::Prune,
            };
            commands::history(&ctx, cmd, format).await
        }
        Command::List {
            filter,
            at,
            sort,
            desc,
            limit,
            format,
        } => {
            let limit = (limit > 0).then_some(limit);
            commands::list(
                &ctx,
                ListOpts {
                    filter,
                    at,
                    sort,
                    desc,
                    limit,
                    format,
                    output: None,
                },
            )
            .await
        }
        Command::Show {
            norad_id,
            at,
            track,
            step,
            format,
        } => {
            commands::show(
                &ctx,
                ShowOpts {
                    norad_id,
                    at,
                    track,
                    step,
                    format,
                },
            )
            .await
        }
        Command::Export {
            filter,
            at,
            format,
            output,
            sort,
        } => {
            if format == Format::Table {
                anyhow::bail!("export 只支持 csv 或 json");
            }
            commands::list(
                &ctx,
                ListOpts {
                    filter,
                    at,
                    sort,
                    desc: false,
                    limit: None,
                    format,
                    output,
                },
            )
            .await
        }
        Command::Stats {
            filter,
            at,
            bin,
            format,
        } => commands::stats(&ctx, filter, at, bin, format).await,
        Command::Serve { bind, sim_time } => commands::serve(&ctx, ServeOpts { bind, sim_time }).await,
        Command::Config {
            action: ConfigAction::Show,
        } => commands::config_show(&ctx),
        Command::Config {
            action: ConfigAction::Init { .. },
        } => unreachable!("已在上方处理"),
    }
}
