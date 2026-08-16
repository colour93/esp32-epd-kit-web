use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{
    producer::{
        BuiltInSourceManifest, ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger,
    },
    publisher::SemanticResource,
    state::unix_now,
};

const SOURCE_ID: &str = "cc-switch";
const RESOURCE_KEY: &str = "ccswitch/metrics";
const POLL_INTERVAL_SEC: u64 = 300;
const RESOURCE_TTL_SEC: u64 = 900;

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "ccswitch.usage",
    title: "CC Switch Usage",
    description: "读取本机 CC Switch 的今日 Token 用量",
    configurable: false,
    multi_instance: false,
    auto_sync: true,
    built_in_source: Some(BuiltInSourceManifest {
        id: SOURCE_ID,
        title: "CC Switch Usage",
        resource_keys: &[RESOURCE_KEY],
    }),
};

#[derive(Clone)]
pub struct CcSwitchControl {
    trigger: mpsc::Sender<ProducerTrigger>,
}

impl CcSwitchControl {
    pub fn spawn(context: ProducerContext) -> Self {
        let (trigger, receiver) = mpsc::channel(8);
        tokio::spawn(run(context, receiver));
        Self { trigger }
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::new(&MANIFEST, self.trigger.clone())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TodayUsage {
    request_count: u64,
    token_count: u64,
}

#[derive(Debug)]
struct ReadFailure {
    phase: &'static str,
    message: String,
}

impl ReadFailure {
    fn missing(message: impl Into<String>) -> Self {
        Self {
            phase: "missing",
            message: message.into(),
        }
    }

    fn degraded(message: impl Into<String>) -> Self {
        Self {
            phase: "degraded",
            message: message.into(),
        }
    }
}

impl Display for ReadFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ReadFailure {}

async fn run(context: ProducerContext, mut triggers: mpsc::Receiver<ProducerTrigger>) {
    let mut poll = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SEC));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll.tick().await;

    loop {
        let mut cycle_id = None;
        let reason = tokio::select! {
            _ = poll.tick() => "poll",
            trigger = triggers.recv() => {
                match trigger {
                    Some(ProducerTrigger::Manual) => "manual",
                    Some(ProducerTrigger::SyncCycle(id)) => {
                        cycle_id = Some(id);
                        "sync_cycle"
                    }
                    None => return,
                }
            }
        };

        context
            .state
            .update_source(SOURCE_ID, |source| {
                source.phase = "syncing".into();
                source.last_error = None;
            })
            .await;
        context
            .state
            .log(
                "info",
                "ccswitch",
                format!("reading today's usage; reason={reason}"),
            )
            .await;

        let result = sync_once(&context).await;
        if let Some(cycle_id) = cycle_id
            && let Err(error) = context
                .publisher
                .complete_cycle(cycle_id, MANIFEST.id, result.is_ok())
                .await
        {
            context
                .state
                .log("warn", "ccswitch", error.to_string())
                .await;
        }

        if let Err(error) = result {
            context
                .state
                .update_source(SOURCE_ID, |source| {
                    source.phase = error.phase.into();
                    source.last_error = Some(error.to_string());
                    source.next_sync_at = Some(unix_now() + POLL_INTERVAL_SEC);
                })
                .await;
            context
                .state
                .log("warn", "ccswitch", error.to_string())
                .await;
        }
    }
}

async fn sync_once(context: &ProducerContext) -> std::result::Result<(), ReadFailure> {
    let path = database_path()?;
    let query_path = path.clone();
    let usage = tokio::task::spawn_blocking(move || read_today_usage(&query_path))
        .await
        .map_err(|error| ReadFailure::degraded(format!("CC Switch 查询任务失败: {error}")))??;

    context
        .publisher
        .publish(SemanticResource {
            source_id: SOURCE_ID.into(),
            key: RESOURCE_KEY.into(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "snapshot",
            payload: json!({
                "source_status": "ok",
                "title": "CC Switch",
                "items": [{
                    "label": "今日 Token",
                    "data": humanize_tokens(usage.token_count),
                    "description": format!("{} 次请求", usage.request_count),
                    "format": "text",
                }],
            }),
        })
        .await
        .map_err(|error| ReadFailure::degraded(format!("发布 CC Switch 用量失败: {error:#}")))?;

    let now = unix_now();
    context
        .state
        .update_source(SOURCE_ID, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + POLL_INTERVAL_SEC);
            source.last_error = None;
            source.details = json!({
                "database_path": path.display().to_string(),
                "request_count": usage.request_count,
                "token_count": usage.token_count,
            });
        })
        .await;
    context
        .state
        .log(
            "info",
            "ccswitch",
            format!(
                "today's usage ready; requests={} tokens={}",
                usage.request_count, usage.token_count
            ),
        )
        .await;
    Ok(())
}

fn database_path() -> std::result::Result<PathBuf, ReadFailure> {
    if let Some(value) = std::env::var_os("CC_SWITCH_DB") {
        if value.is_empty() {
            return Err(ReadFailure::degraded("CC_SWITCH_DB 不能为空"));
        }
        return Ok(PathBuf::from(value));
    }
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|home| home.join(".cc-switch").join("cc-switch.db"))
        .ok_or_else(|| ReadFailure::missing("无法确定用户目录，找不到 CC Switch 数据库"))
}

fn read_today_usage(path: &Path) -> std::result::Result<TodayUsage, ReadFailure> {
    if !path.is_file() {
        return Err(ReadFailure::missing(format!(
            "找不到 CC Switch 数据库: {}",
            path.display()
        )));
    }

    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        ReadFailure::degraded(format!(
            "无法只读打开 CC Switch 数据库 {}: {error}",
            path.display()
        ))
    })?;
    let has_request_logs = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'proxy_request_logs')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .context("检查 proxy_request_logs 表")
        .map_err(|error| ReadFailure::degraded(error.to_string()))?;
    if !has_request_logs {
        return Err(ReadFailure::missing(
            "CC Switch 数据库缺少 proxy_request_logs 表",
        ));
    }

    let (request_count, token_count) = connection
        .query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(
                    COALESCE(input_tokens, 0) +
                    COALESCE(output_tokens, 0) +
                    COALESCE(cache_read_tokens, 0) +
                    COALESCE(cache_creation_tokens, 0)
                ), 0)
             FROM proxy_request_logs
             WHERE date(created_at, 'unixepoch', 'localtime') = date('now', 'localtime')",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .context("聚合 CC Switch 今日 Token 用量")
        .map_err(|error| ReadFailure::degraded(error.to_string()))?;

    Ok(TodayUsage {
        request_count: request_count
            .try_into()
            .map_err(|_| ReadFailure::degraded("CC Switch 今日请求数无效"))?,
        token_count: token_count
            .try_into()
            .map_err(|_| ReadFailure::degraded("CC Switch 今日 Token 总量无效"))?,
    })
}

fn humanize_tokens(value: u64) -> String {
    const UNITS: [&str; 7] = ["", "K", "M", "B", "T", "P", "E"];

    if value < 1_000 {
        return value.to_string();
    }

    let mut unit = 0usize;
    let mut scaled = value as f64;
    while scaled >= 1_000.0 && unit + 1 < UNITS.len() {
        scaled /= 1_000.0;
        unit += 1;
    }
    let mut rounded = (scaled * 10.0).round() / 10.0;
    if rounded >= 1_000.0 && unit + 1 < UNITS.len() {
        rounded /= 1_000.0;
        unit += 1;
    }

    let number = if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    };
    format!("{number}{}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;
    use uuid::Uuid;

    use super::{ReadFailure, TodayUsage, humanize_tokens, read_today_usage};

    struct DatabaseFixture {
        path: PathBuf,
    }

    impl DatabaseFixture {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir()
                    .join(format!("epd-agent-ccswitch-{}.db", Uuid::new_v4())),
            }
        }
    }

    impl Drop for DatabaseFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn humanizes_decimal_token_counts() {
        assert_eq!(humanize_tokens(999), "999");
        assert_eq!(humanize_tokens(1_000), "1K");
        assert_eq!(humanize_tokens(64_493), "64.5K");
        assert_eq!(humanize_tokens(1_250_000), "1.3M");
        assert_eq!(humanize_tokens(999_999), "1M");
    }

    #[test]
    fn reads_only_local_today_from_request_logs() {
        let fixture = DatabaseFixture::new();
        let connection = Connection::open(&fixture.path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE proxy_request_logs (
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_creation_tokens INTEGER,
                    created_at INTEGER NOT NULL
                );
                INSERT INTO proxy_request_logs VALUES (47000, 400, 16000, 0, strftime('%s', 'now'));
                INSERT INTO proxy_request_logs VALUES (247, 78, 768, NULL, strftime('%s', 'now'));
                INSERT INTO proxy_request_logs VALUES (999, 999, 999, 999, strftime('%s', 'now', '-2 days'));",
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            read_today_usage(&fixture.path).unwrap(),
            TodayUsage {
                request_count: 2,
                token_count: 64_493,
            }
        );
    }

    #[test]
    fn reports_missing_request_log_table() {
        let fixture = DatabaseFixture::new();
        drop(Connection::open(&fixture.path).unwrap());

        let ReadFailure { phase, message } = read_today_usage(&fixture.path).unwrap_err();
        assert_eq!(phase, "missing");
        assert!(message.contains("proxy_request_logs"));
    }
}
