export type JsonObject = Record<string, unknown>

export interface PageBinding {
  widget_id: string
  resource_key: string
}

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
  page: { id: string; bindings: Record<string, PageBinding | string> }
}

export interface PageWidgetCapability {
  id: string
  title: string
  schema_id: string
  schema_version: number
}

export interface PageSlotCapability {
  id: string
  title?: string
  status: 'active' | 'reserved'
  required: boolean
  widgets?: PageWidgetCapability[]
  schema_id?: string
  schema_version?: number
}

export interface PageCapability {
  id: string
  title: string
  slots: PageSlotCapability[]
  timed_regions: Array<{
    id: string
    interval_sec: number
    bounds: { x: number; y: number; width: number; height: number }
  }>
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
    pages?: PageCapability[]
    battery?: boolean
    io12?: boolean
    max_resources?: number
    max_resource_payload_bytes?: number
    max_page_bindings?: number
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

export interface ProducerStatus {
  id: string
  title: string
  phase: string
  resource_keys: string[]
  last_sync_at?: number
  next_sync_at?: number
  last_error?: string
  details: JsonObject
}

export interface FeishuProjectConfig {
  enabled: boolean
  display_name: string
  command: string
  value_expression: string
  detail_expression: string
}

export interface FeishuProjectPreview {
  source_status: 'ok'
  display_name: string
  value: string
  detail?: string
  elapsed_ms: number
  output_bytes: number
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
  producers: ProducerStatus[]
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
  refreshProducer: (id: string) => request(`/api/v1/producers/${encodeURIComponent(id)}/refresh`, {
    method: 'POST', body: '{}',
  }),
  getFeishuProjectConfig: () => request<{ config: FeishuProjectConfig }>(
    '/api/v1/producers/feishu.project/config',
  ),
  saveFeishuProjectConfig: (config: FeishuProjectConfig) => request<{ config: FeishuProjectConfig }>(
    '/api/v1/producers/feishu.project/config',
    { method: 'PUT', body: JSON.stringify(config) },
  ),
  testFeishuProjectConfig: (config: FeishuProjectConfig) => request<{ preview: FeishuProjectPreview }>(
    '/api/v1/producers/feishu.project/test',
    { method: 'POST', body: JSON.stringify(config) },
  ),
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
  putResource: (resource: JsonObject) => request('/api/v1/device/resource', {
    method: 'PUT', body: JSON.stringify({ resource }),
  }),
  setPage: (page: { id: string; bindings: Record<string, PageBinding | string> }) => request('/api/v1/device/page', {
    method: 'POST', body: JSON.stringify({ page }),
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
