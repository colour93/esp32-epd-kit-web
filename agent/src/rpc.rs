use std::{
    fmt,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{
    protocol::{self, FrameAssembler, MessageKind},
    state::SharedState,
};

#[derive(Debug)]
pub struct LinkError(String);

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LinkError {}

pub fn link_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(LinkError(message.into()))
}

pub fn is_link_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<LinkError>().is_some()
}

#[allow(async_fn_in_trait)]
pub trait FrameChannel {
    fn transport_name(&self) -> &'static str;
    fn frame_bytes(&self) -> usize;
    fn next_request_id(&self) -> u32;
    fn assembler(&mut self) -> &mut FrameAssembler;

    async fn write_frame(&mut self, frame: &[u8]) -> Result<()>;
    async fn read_frame(&mut self) -> Result<Vec<u8>>;
}

pub async fn transact<C: FrameChannel>(
    state: &SharedState,
    channel: &mut C,
    device_events: &broadcast::Sender<String>,
    op: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value> {
    let transport = channel.transport_name();
    let id = channel.next_request_id();
    let frames = protocol::encode_request(id, op, args, channel.frame_bytes())?;
    let frame_count = frames.len();
    let encoded_bytes = frames.iter().map(Vec::len).sum::<usize>();
    let started = Instant::now();
    state
        .log(
            "debug",
            "device.rpc",
            format!(
                "transport={transport} request id={id} op={op} frames={frame_count} bytes={encoded_bytes}"
            ),
        )
        .await;
    for frame in frames {
        tokio::time::timeout(timeout, channel.write_frame(&frame))
            .await
            .map_err(|_| link_error(format!("{transport} write timed out for {op}")))?
            .map_err(|error| link_error(format!("write {transport} request {op}: {error}")))?;
    }
    let response = tokio::time::timeout(timeout, async {
        loop {
            let frame = channel.read_frame().await?;
            let Some(message) = channel.assembler().feed(&frame)? else {
                continue;
            };
            match message.kind {
                MessageKind::Response if message.id == id => {
                    return Ok::<protocol::Response, anyhow::Error>(protocol::decode_response(
                        &message.payload,
                    )?);
                }
                MessageKind::Event => {
                    dispatch_event(state, device_events, &message.payload).await?
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| link_error(format!("{transport} response timed out for {op}")))?
    .map_err(|error| link_error(format!("{transport} response failed for {op}: {error:#}")))?;
    if response.ok {
        state
            .log(
                "debug",
                "device.rpc",
                format!(
                    "transport={transport} response id={id} op={op} elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
            )
            .await;
        return Ok(response.result);
    }
    let error = response
        .error
        .ok_or_else(|| anyhow!("device request failed without error payload"))?;
    state
        .log(
            "warn",
            "device.rpc",
            format!(
                "transport={transport} response id={id} op={op} code={} elapsed_ms={}",
                error.code,
                started.elapsed().as_millis()
            ),
        )
        .await;
    Err(anyhow!("{}: {}", error.code, error.message))
}

pub async fn handle_frame(
    state: &SharedState,
    assembler: &mut FrameAssembler,
    device_events: &broadcast::Sender<String>,
    value: &[u8],
) -> Result<()> {
    let Some(message) = assembler.feed(value)? else {
        return Ok(());
    };
    if message.kind == MessageKind::Event {
        dispatch_event(state, device_events, &message.payload).await?;
    }
    Ok(())
}

async fn dispatch_event(
    state: &SharedState,
    device_events: &broadcast::Sender<String>,
    payload: &[u8],
) -> Result<()> {
    let event = protocol::decode_event(payload)?;
    state
        .log("info", "device", format!("{} {}", event.name, event.data))
        .await;
    let _ = device_events.send(event.name);
    Ok(())
}
