use std::{
    collections::{HashMap, HashSet},
    io::SeekFrom,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc,
};

use crate::{
    codex::{AppServer, find_codex},
    producer::{
        BuiltInSourceManifest, ProducerContext, ProducerControl, ProducerManifest, ProducerTrigger,
    },
    publisher::SemanticResource,
    state::unix_now,
};

const SOURCE_ID: &str = "codex-tasks";
const RESOURCE_KEY: &str = "codex/tasks";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const PUBLISH_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const RESOURCE_TTL_SEC: u64 = 30;
const MAX_TASKS: usize = 4;

pub static MANIFEST: ProducerManifest = ProducerManifest {
    id: "codex.tasks",
    title: "Codex Tasks",
    description: "监视本机 Codex 最近任务的执行状态",
    configurable: false,
    multi_instance: false,
    auto_sync: true,
    built_in_source: Some(BuiltInSourceManifest {
        id: SOURCE_ID,
        title: "Codex Tasks",
        resource_keys: &[RESOURCE_KEY],
    }),
};

#[derive(Clone)]
pub struct CodexTaskControl {
    trigger: mpsc::Sender<ProducerTrigger>,
}

impl CodexTaskControl {
    pub fn spawn(context: ProducerContext) -> Self {
        let (trigger, receiver) = mpsc::channel(8);
        tokio::spawn(supervisor(context, receiver));
        Self { trigger }
    }

    pub fn control(&self) -> ProducerControl {
        ProducerControl::new(&MANIFEST, self.trigger.clone())
    }
}

#[derive(Default)]
struct RolloutCursor {
    path: PathBuf,
    offset: u64,
    pending: String,
    status: Option<&'static str>,
}

#[derive(Default)]
struct TaskTracker {
    rollouts: HashMap<String, RolloutCursor>,
}

impl TaskTracker {
    async fn update(&mut self, thread: &Value) -> Result<Option<Value>> {
        let id = thread
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("thread/list item has no id"))?;
        let path = thread
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("thread {id} has no rollout path"))?;
        let cursor = self.rollouts.entry(id.to_owned()).or_default();
        ingest_rollout(cursor, path).await?;
        let Some(status) = cursor.status else {
            return Ok(None);
        };
        let title = thread
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| thread.get("preview").and_then(Value::as_str))
            .unwrap_or("未命名任务");
        let project = thread
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|cwd| PathBuf::from(cwd).file_name().map(|name| name.to_owned()))
            .and_then(|name| name.to_str().map(str::to_owned))
            .unwrap_or_else(|| "Codex".into());
        Ok(Some(json!({
            "label": truncate(&project, 24),
            "data": status_text(status),
            "description": truncate(title, 64),
            "format": "text",
            "running": status == "running",
        })))
    }
}

async fn supervisor(context: ProducerContext, mut triggers: mpsc::Receiver<ProducerTrigger>) {
    let mut backoff = 1u64;
    loop {
        let path = match find_codex() {
            Ok(path) => path,
            Err(error) => {
                set_error(&context, "missing", error.to_string()).await;
                wait_or_complete(&context, &mut triggers, 30).await;
                continue;
            }
        };
        match AppServer::start(&path, context.state.clone(), "codex.tasks").await {
            Ok(mut server) => {
                if let Err(error) = run_connected(&context, &mut server, &mut triggers).await {
                    let message = format!("{error:#}");
                    set_error(&context, "unavailable", message.clone()).await;
                    context.state.log("warn", "codex.tasks", message).await;
                }
            }
            Err(error) => {
                set_error(&context, "unavailable", error.to_string()).await;
            }
        }
        wait_or_complete(&context, &mut triggers, backoff).await;
        backoff = (backoff * 2).min(60);
    }
}

async fn run_connected(
    context: &ProducerContext,
    server: &mut AppServer,
    triggers: &mut mpsc::Receiver<ProducerTrigger>,
) -> Result<()> {
    let mut tracker = TaskTracker::default();
    let mut failed_payload: Option<Value> = None;
    let mut next_publish_attempt = tokio::time::Instant::now();
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let (cycle_id, force_publish) = tokio::select! {
            _ = poll.tick() => (None, false),
            trigger = triggers.recv() => match trigger {
                Some(ProducerTrigger::Manual) => (None, true),
                Some(ProducerTrigger::SyncCycle(id)) => (Some(id), true),
                None => return Err(anyhow!("producer control channel closed")),
            },
        };
        let collected = collect_tasks(context, server, &mut tracker).await?;
        let should_publish = force_publish
            || failed_payload.as_ref() != Some(&collected.payload)
            || tokio::time::Instant::now() >= next_publish_attempt;
        let success = if should_publish {
            match publish_tasks(context, &collected).await {
                Ok(()) => {
                    failed_payload = None;
                    true
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    set_error(context, "degraded", message.clone()).await;
                    context.state.log("warn", "codex.tasks", message).await;
                    failed_payload = Some(collected.payload);
                    next_publish_attempt = tokio::time::Instant::now() + PUBLISH_RETRY_INTERVAL;
                    false
                }
            }
        } else {
            false
        };
        if let Some(cycle_id) = cycle_id {
            context
                .publisher
                .complete_cycle(cycle_id, MANIFEST.id, success)
                .await?;
        }
    }
}

struct CollectedTasks {
    payload: Value,
    task_count: usize,
}

async fn collect_tasks(
    context: &ProducerContext,
    server: &mut AppServer,
    tracker: &mut TaskTracker,
) -> Result<CollectedTasks> {
    let result = server
        .request(
            "thread/list",
            json!({ "limit": MAX_TASKS, "sortKey": "updated_at" }),
        )
        .await
        .context("list Codex tasks")?;
    let threads = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("thread/list response has no data"))?;
    let visible_ids = threads
        .iter()
        .filter_map(|thread| thread.get("id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    tracker
        .rollouts
        .retain(|thread_id, _| visible_ids.contains(thread_id.as_str()));
    let mut items = Vec::new();
    for thread in threads {
        match tracker.update(thread).await {
            Ok(Some(item)) => items.push(item),
            Ok(None) => {}
            Err(error) => {
                context
                    .state
                    .log("debug", "codex.tasks", error.to_string())
                    .await;
            }
        }
    }
    items.sort_by_key(|item| {
        !item
            .get("running")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    for item in &mut items {
        item.as_object_mut().map(|object| object.remove("running"));
    }
    if items.is_empty() {
        items.push(json!({
            "label": "状态",
            "data": "暂无任务",
            "description": "本机 Codex",
            "format": "text",
        }));
    }
    Ok(CollectedTasks {
        payload: json!({
            "source_status": "ok",
            "title": "Codex 任务",
            "items": items,
        }),
        task_count: threads.len(),
    })
}

async fn publish_tasks(context: &ProducerContext, collected: &CollectedTasks) -> Result<()> {
    context
        .publisher
        .publish(SemanticResource {
            source_id: SOURCE_ID.into(),
            key: RESOURCE_KEY.into(),
            schema_id: "generic.metrics",
            schema_version: 1,
            ttl_sec: RESOURCE_TTL_SEC,
            persistence: "volatile",
            payload: collected.payload.clone(),
        })
        .await?;
    let now = unix_now();
    context
        .state
        .update_source(SOURCE_ID, |source| {
            source.phase = "ready".into();
            source.last_sync_at = Some(now);
            source.next_sync_at = Some(now + POLL_INTERVAL.as_secs());
            source.last_error = None;
            source.details["poll_interval_sec"] = json!(POLL_INTERVAL.as_secs());
            source.details["task_count"] = json!(collected.task_count);
        })
        .await;
    Ok(())
}

async fn ingest_rollout(cursor: &mut RolloutCursor, path: PathBuf) -> Result<()> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("read rollout metadata {}", path.display()))?;
    if cursor.path != path || metadata.len() < cursor.offset {
        *cursor = RolloutCursor {
            path: path.clone(),
            ..Default::default()
        };
    }
    if metadata.len() == cursor.offset {
        return Ok(());
    }
    let mut file = File::open(&path).await?;
    file.seek(SeekFrom::Start(cursor.offset)).await?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await?;
    cursor.offset = metadata.len();
    let chunk = format!("{}{}", cursor.pending, String::from_utf8_lossy(&bytes));
    let (complete, pending) = chunk.rsplit_once('\n').unwrap_or(("", chunk.as_str()));
    cursor.pending = pending.to_owned();
    for line in complete.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        match record
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
        {
            Some("task_started") => cursor.status = Some("running"),
            Some("task_complete") => cursor.status = Some("completed"),
            Some("turn_aborted") => cursor.status = Some("interrupted"),
            _ => {}
        }
    }
    Ok(())
}

async fn wait_or_complete(
    context: &ProducerContext,
    triggers: &mut mpsc::Receiver<ProducerTrigger>,
    delay_sec: u64,
) {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(delay_sec)) => {}
        trigger = triggers.recv() => {
            if let Some(ProducerTrigger::SyncCycle(cycle_id)) = trigger {
                let _ = context.publisher.complete_cycle(cycle_id, MANIFEST.id, false).await;
            }
        }
    }
}

async fn set_error(context: &ProducerContext, phase: &str, error: String) {
    context
        .state
        .update_source(SOURCE_ID, |source| {
            source.phase = phase.into();
            source.last_error = Some(error);
        })
        .await;
}

fn status_text(status: &str) -> &'static str {
    match status {
        "running" => "执行中",
        "interrupted" => "已中止",
        _ => "已完成",
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.pop();
        result.push('…');
    }
    result
}
