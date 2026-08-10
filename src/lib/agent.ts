export type JsonObject = Record<string, unknown>

export interface AgentStatus {
  version: string
  paused: boolean
  platform: string
  autostart_enabled: boolean
}

export interface DeviceConfig {
  version: number
  revision: number
  device: { name: string; locale: string; timezone_iana: string }
  hardware: {
    battery: { enabled: boolean; low_mv: number; critical_mv: number; recovery_mv: number }
    io12: { mode: 'disabled' | 'key' }
  }
  power: { profile: 'mains' | 'battery'; wake_interval_sec: number }
  display: {
    full_after_partial_count: number
    full_max_age_sec: number
    full_area_threshold_percent: number
  }
  view: { renderer_id: string; resource_key: string }
}

export interface RendererCapability {
  id: string
  schema_id: string
  schema_version: number
}

export interface DeviceStatus {
  phase: string
  connection_mode: 'auto' | 'scan' | 'manual' | 'idle'
  preferred_device_id?: string
  selected_device_id?: string
  candidates: BleCandidate[]
  scan_observed: number
  scan_started_at?: number
  name?: string
  role?: 'owner' | 'trusted'
  firmware?: string
  mtu?: number
  config?: DeviceConfig
  capabilities?: {
    renderers?: RendererCapability[]
    battery?: boolean
    io12?: boolean
    max_resources?: number
    max_resource_payload_bytes?: number
  }
  resources: ResourceSummary[]
  bonds: Bond[]
  diagnostics?: JsonObject
  last_error?: string
}

export interface BleCandidate {
  id: string
  name: string
  rssi?: number
  advertises_service: boolean
  protocol_major?: number
  owned?: boolean
  battery?: boolean
  fast_advertising?: boolean
  last_seen_at: number
}

export interface ResourceSummary {
  key: string
  schema_id: string
  schema_version: number
  revision: number
  updated_at: number
  ttl_sec: number
  persistence: 'volatile' | 'snapshot'
  content_crc: number
}

export interface Bond {
  id: string
  role: 'owner' | 'trusted'
}

export interface CodexStatus {
  phase: string
  account_type?: string
  email?: string
  plan_type?: string
  codex_path?: string
  last_sync_at?: number
  next_sync_at?: number
  last_error?: string
  rate_limits?: JsonObject
}

export interface LogEntry {
  at: number
  level: 'info' | 'warn' | 'error'
  scope: string
  message: string
}

export interface Snapshot {
  agent: AgentStatus
  device: DeviceStatus
  codex: CodexStatus
  logs: LogEntry[]
}

export class AgentApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.name = 'AgentApiError'
    this.status = status
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: 'same-origin',
    ...init,
    headers: { 'Content-Type': 'application/json', ...init?.headers },
  })
  const body = await response.json().catch(() => ({})) as { error?: { message?: string } }
  if (!response.ok) {
    throw new AgentApiError(response.status, body.error?.message ?? `Agent HTTP ${response.status}`)
  }
  return body as T
}

export async function establishSession() {
  const hash = new URLSearchParams(window.location.hash.slice(1))
  const token = hash.get('token')
  if (!token) return
  window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`)
  await request('/api/v1/session', { method: 'POST', body: JSON.stringify({ token }) })
}

export const agentApi = {
  snapshot: () => request<Snapshot>('/api/v1/snapshot'),
  scanDevices: () => request('/api/v1/device/scan', { method: 'POST', body: '{}' }),
  connectDevice: (id: string) => request('/api/v1/device/connect', {
    method: 'POST', body: JSON.stringify({ id }),
  }),
  disconnectDevice: () => request('/api/v1/device/disconnect', { method: 'POST', body: '{}' }),
  autoConnectDevice: () => request('/api/v1/device/auto-connect', { method: 'POST', body: '{}' }),
  reloadDevice: () => request('/api/v1/device/reload', { method: 'POST', body: '{}' }),
  patchConfig: (patch: JsonObject) => request<{ result: { revision: number; restart_required: boolean } }>(
    '/api/v1/device/config',
    { method: 'PATCH', body: JSON.stringify({ patch }) },
  ),
  refreshDisplay: (mode: 'auto' | 'full') => request('/api/v1/device/refresh', {
    method: 'POST', body: JSON.stringify({ mode }),
  }),
  restartDevice: () => request('/api/v1/device/restart', { method: 'POST', body: '{}' }),
  refreshCodex: () => request('/api/v1/codex/refresh', { method: 'POST', body: '{}' }),
  setPaused: (enabled: boolean) => request('/api/v1/agent/pause', {
    method: 'POST', body: JSON.stringify({ enabled }),
  }),
  setAutostart: (enabled: boolean) => request('/api/v1/agent/autostart', {
    method: 'POST', body: JSON.stringify({ enabled }),
  }),
  setEnrollment: (enabled: boolean) => request<{ result: { expires_in_sec?: number } }>(
    '/api/v1/security/enrollment',
    { method: 'POST', body: JSON.stringify({ enabled }) },
  ),
  revokeBond: (id: string) => request(`/api/v1/security/bonds/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  transferOwner: (id: string) => request(`/api/v1/security/owner/${encodeURIComponent(id)}`, {
    method: 'POST', body: '{}',
  }),
  getResource: (key: string) => request<{ result: { resource: JsonObject } }>(
    `/api/v1/device/resource?key=${encodeURIComponent(key)}`,
  ),
  deleteResource: (key: string) => request(`/api/v1/device/resource?key=${encodeURIComponent(key)}`, {
    method: 'DELETE',
  }),
  setView: (rendererId: string, resourceKey: string) => request('/api/v1/device/view', {
    method: 'POST', body: JSON.stringify({ renderer_id: rendererId, resource_key: resourceKey }),
  }),
  prepareFactoryReset: () => request<{ result: { expires_in_sec: number } }>('/api/v1/factory/prepare', {
    method: 'POST', body: '{}',
  }),
  commitFactoryReset: (code: number) => request('/api/v1/factory/commit', {
    method: 'POST', body: JSON.stringify({ code }),
  }),
}

export function subscribeSnapshots(onSnapshot: (snapshot: Snapshot) => void, onError: () => void) {
  const source = new EventSource('/api/v1/events')
  source.addEventListener('snapshot', (event) => {
    onSnapshot(JSON.parse((event as MessageEvent<string>).data) as Snapshot)
  })
  source.onerror = onError
  return () => source.close()
}
