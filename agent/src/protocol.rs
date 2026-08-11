use anyhow::{Context, Result, anyhow, bail};
use crc32fast::hash;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

pub const SERVICE_UUID: uuid::Uuid = uuid::uuid!("f0a40000-0451-4000-b000-000000000001");
pub const RX_UUID: uuid::Uuid = uuid::uuid!("f0a40001-0451-4000-b000-000000000001");
pub const TX_UUID: uuid::Uuid = uuid::uuid!("f0a40002-0451-4000-b000-000000000001");
pub const FRAME_MAGIC: u8 = 0xe4;
pub const MAX_MESSAGE_BYTES: usize = 8192;
const HEADER_BYTES: usize = 8;
const START_METADATA_BYTES: usize = 6;
const FLAG_KIND_MASK: u8 = 0x03;
const FLAG_START: u8 = 0x04;
const FLAG_END: u8 = 0x08;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageKind {
    Request = 0,
    Response = 1,
    Event = 2,
}

#[derive(Debug, Serialize)]
pub struct Request<'a> {
    pub op: &'a str,
    pub args: Value,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    pub name: String,
    #[serde(default)]
    pub data: Value,
}

pub fn encode_request(id: u32, op: &str, args: Value, frame_bytes: usize) -> Result<Vec<Vec<u8>>> {
    let payload = rmp_serde::to_vec_named(&Request { op, args })?;
    encode_message(id, MessageKind::Request, &payload, frame_bytes)
}

pub fn encode_message(
    id: u32,
    kind: MessageKind,
    payload: &[u8],
    frame_bytes: usize,
) -> Result<Vec<Vec<u8>>> {
    if payload.is_empty() || payload.len() > MAX_MESSAGE_BYTES {
        bail!("message length is outside BLE v4 limits");
    }
    if frame_bytes < HEADER_BYTES + START_METADATA_BYTES {
        bail!("GATT frame is too small");
    }
    let checksum = hash(payload);
    let mut sequence = 0u16;
    let mut offset = 0usize;
    let mut frames = Vec::new();
    loop {
        let start = sequence == 0;
        let metadata = if start { START_METADATA_BYTES } else { 0 };
        let capacity = frame_bytes - HEADER_BYTES - metadata;
        let chunk = capacity.min(payload.len() - offset);
        let end = offset + chunk == payload.len();
        let mut frame = vec![0u8; HEADER_BYTES + metadata + chunk];
        frame[0] = FRAME_MAGIC;
        frame[1] = kind as u8 | if start { FLAG_START } else { 0 } | if end { FLAG_END } else { 0 };
        frame[2..6].copy_from_slice(&id.to_le_bytes());
        frame[6..8].copy_from_slice(&sequence.to_le_bytes());
        let mut write_at = HEADER_BYTES;
        if start {
            frame[write_at..write_at + 2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
            frame[write_at + 2..write_at + 6].copy_from_slice(&checksum.to_le_bytes());
            write_at += START_METADATA_BYTES;
        }
        frame[write_at..].copy_from_slice(&payload[offset..offset + chunk]);
        frames.push(frame);
        offset += chunk;
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| anyhow!("too many fragments"))?;
        if end {
            break;
        }
    }
    Ok(frames)
}

#[derive(Debug, Default)]
pub struct FrameAssembler {
    active: bool,
    id: u32,
    kind: u8,
    next_sequence: u16,
    total_length: usize,
    expected_crc: u32,
    payload: Vec<u8>,
    started_at: Option<Instant>,
}

#[derive(Debug)]
pub struct CompleteMessage {
    pub id: u32,
    pub kind: MessageKind,
    pub payload: Vec<u8>,
}

impl FrameAssembler {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn feed(&mut self, frame: &[u8]) -> Result<Option<CompleteMessage>> {
        if self.active
            && self
                .started_at
                .is_some_and(|started| started.elapsed() > Duration::from_secs(5))
        {
            self.clear();
            bail!("BLE v4 fragment assembly timed out");
        }
        if frame.len() < HEADER_BYTES || frame[0] != FRAME_MAGIC {
            bail!("invalid BLE v4 frame header");
        }
        let flags = frame[1];
        let kind = flags & FLAG_KIND_MASK;
        if kind > MessageKind::Event as u8 {
            bail!("invalid message kind");
        }
        let id = u32::from_le_bytes(frame[2..6].try_into().unwrap());
        let sequence = u16::from_le_bytes(frame[6..8].try_into().unwrap());
        let mut offset = HEADER_BYTES;
        if flags & FLAG_START != 0 {
            if sequence != 0 || frame.len() < HEADER_BYTES + START_METADATA_BYTES {
                bail!("invalid start frame");
            }
            self.clear();
            self.active = true;
            self.started_at = Some(Instant::now());
            self.id = id;
            self.kind = kind;
            self.total_length =
                u16::from_le_bytes(frame[offset..offset + 2].try_into().unwrap()) as usize;
            self.expected_crc =
                u32::from_le_bytes(frame[offset + 2..offset + 6].try_into().unwrap());
            self.payload.reserve(self.total_length);
            offset += START_METADATA_BYTES;
            if self.total_length == 0 || self.total_length > MAX_MESSAGE_BYTES {
                self.clear();
                bail!("declared message length is invalid");
            }
        }
        if !self.active || self.id != id || self.kind != kind || self.next_sequence != sequence {
            self.clear();
            bail!("fragment sequence is not contiguous");
        }
        self.next_sequence += 1;
        self.payload.extend_from_slice(&frame[offset..]);
        if self.payload.len() > self.total_length {
            self.clear();
            bail!("message exceeds declared length");
        }
        if flags & FLAG_END == 0 {
            return Ok(None);
        }
        if self.payload.len() != self.total_length || hash(&self.payload) != self.expected_crc {
            self.clear();
            bail!("message length or CRC mismatch");
        }
        let payload = std::mem::take(&mut self.payload);
        let kind = match self.kind {
            0 => MessageKind::Request,
            1 => MessageKind::Response,
            2 => MessageKind::Event,
            _ => unreachable!(),
        };
        let message = CompleteMessage {
            id: self.id,
            kind,
            payload,
        };
        self.clear();
        Ok(Some(message))
    }
}

pub fn decode_response(payload: &[u8]) -> Result<Response> {
    rmp_serde::from_slice(payload).context("decode BLE response")
}

pub fn decode_event(payload: &[u8]) -> Result<Event> {
    rmp_serde::from_slice(payload).context("decode BLE event")
}
