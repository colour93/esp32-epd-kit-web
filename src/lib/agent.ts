export type JsonObject = Record<string, unknown>

export interface PageBinding {
  widget_id: string
  resource_key: string
}

export interface PagePreset {
  id: string
  title: string
  page: { id: string; bindings: Record<string, PageBinding | string> }
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
  widget_ids?: string[]
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
    widgets?: PageWidgetCapability[]
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
  pairing?: PairingStatus
  last_error?: string
}

export interface PairingStatus {
  request_id: string
  device_name: string
  expires_at: number
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

export interface SourceTypeStatus {
  id: string
  title: string
  description: string
  configurable: boolean
  multi_instance: boolean
  auto_sync: boolean
}

export interface SourceStatus {
  id: string
  type_id: string
  title: string
  enabled: boolean
  interval_sec?: number
  realtime: boolean
  phase: string
  resource_keys: string[]
  last_sync_at?: number
  next_sync_at?: number
  last_error?: string
  details: JsonObject
}

export type GenericMetricFormat = 'text' | 'percent' | 'countdown' | 'compact_number'

export interface CliMetricItemConfig {
  label: string
  data_expression: string
  description_expression: string
  progress_expression: string
  format: GenericMetricFormat
}

export interface CliMetricConfig {
  id: string
  enabled: boolean
  title: string
  command: string
  items: CliMetricItemConfig[]
}

export type HttpMetricMethod = 'GET' | 'POST'
export type HttpMetricNetworkAccess = 'public' | 'private' | 'localhost'
export type HttpMetricAuthType = 'none' | 'bearer' | 'header'

export interface HttpMetricHeaderConfig {
  name: string
  value: string
}

export interface HttpMetricAuthConfig {
  type: HttpMetricAuthType
  header_name?: string
  secret?: string
  secret_configured?: boolean
  clear_secret?: boolean
}

export interface HttpMetricConfig {
  id: string
  enabled: boolean
  title: string
  interval_sec: number
  timeout_ms: number
  method: HttpMetricMethod
  url: string
  network_access: HttpMetricNetworkAccess
  headers: HttpMetricHeaderConfig[]
  body: string
  auth: HttpMetricAuthConfig
  items: CliMetricItemConfig[]
}

export interface CliMetricPreviewItem {
  label: string
  data: unknown
  description?: string
  progress?: number
  format: GenericMetricFormat
}

export interface CliMetricPreview {
  source_status: 'ok'
  title: string
  items: CliMetricPreviewItem[]
  elapsed_ms: number
  output_bytes: number
}

export type HttpMetricPreview = CliMetricPreview

export type BalancePlatform = 'deepseek' | 'moonshot'

export interface BalanceConfig {
  id: string
  enabled: boolean
  title: string
  platform: BalancePlatform
  interval_sec: number
  timeout_ms: number
  api_key?: string
  secret_configured?: boolean
  clear_secret?: boolean
}

export interface CodexOAuthConfig {
  id: string
  enabled: boolean
  title: string
  interval_sec: number
  authenticated: boolean
  email?: string
  plan_type?: string
  expires_at?: number
}

export interface CodexOAuthStartResult {
  session_id: string
  auth_url: string
}

export interface LogEntry {
  at: number
  level: 'debug' | 'info' | 'warn' | 'error'
  scope: string
  message: string
}

export interface Snapshot {
  agent: AgentStatus
  device: DeviceStatus
  source_types: SourceTypeStatus[]
  sources: SourceStatus[]
  page_presets: PagePreset[]
  resource_catalog: ResourceSummary[]
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
  submitPairingPin: (requestId: string, pin: string) => request('/api/v1/device/pairing', {
    method: 'POST', body: JSON.stringify({ request_id: requestId, pin }),
  }),
  cancelPairing: (requestId: string) => request('/api/v1/device/pairing', {
    method: 'DELETE', body: JSON.stringify({ request_id: requestId }),
  }),
  reloadDevice: () => request('/api/v1/device/reload', { method: 'POST', body: '{}' }),
  patchConfig: (patch: JsonObject) => request<{ result: { revision: number; restart_required: boolean } }>(
    '/api/v1/device/config',
    { method: 'PATCH', body: JSON.stringify({ patch }) },
  ),
  refreshDisplay: (mode: 'auto' | 'full') => request('/api/v1/device/refresh', {
    method: 'POST', body: JSON.stringify({ mode }),
  }),
  restartDevice: () => request('/api/v1/device/restart', { method: 'POST', body: '{}' }),
  refreshSourceType: (id: string) => request(`/api/v1/source-types/${encodeURIComponent(id)}/refresh`, {
    method: 'POST', body: '{}',
  }),
  refreshSource: (id: string) => request(`/api/v1/sources/${encodeURIComponent(id)}/refresh`, {
    method: 'POST', body: '{}',
  }),
  updateSourcePolicy: (id: string, policy: { enabled?: boolean; interval_sec?: number }) => request(
    `/api/v1/sources/${encodeURIComponent(id)}/policy`,
    { method: 'PATCH', body: JSON.stringify(policy) },
  ),
  getCodexOAuthSources: () => request<{ sources: CodexOAuthConfig[] }>(
    '/api/v1/source-types/codex.oauth/sources',
  ),
  startCodexOAuth: (source: Pick<CodexOAuthConfig, 'id' | 'enabled' | 'title' | 'interval_sec'>) => request<{ result: CodexOAuthStartResult }>(
    '/api/v1/source-types/codex.oauth/oauth/start',
    { method: 'POST', body: JSON.stringify(source) },
  ),
  completeCodexOAuth: (sessionId: string, callbackUrl: string) => request<{ source: CodexOAuthConfig }>(
    '/api/v1/source-types/codex.oauth/oauth/complete',
    { method: 'POST', body: JSON.stringify({ session_id: sessionId, callback_url: callbackUrl }) },
  ),
  updateCodexOAuthSource: (source: Pick<CodexOAuthConfig, 'id' | 'enabled' | 'title' | 'interval_sec'>) => request<{ source: CodexOAuthConfig }>(
    `/api/v1/source-types/codex.oauth/sources/${encodeURIComponent(source.id)}`,
    { method: 'PUT', body: JSON.stringify(source) },
  ),
  deleteCodexOAuthSource: (id: string) => request(
    `/api/v1/source-types/codex.oauth/sources/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  ),
  getCliMetricSources: () => request<{ sources: CliMetricConfig[] }>(
    '/api/v1/source-types/cli.jmespath/sources',
  ),
  createCliMetricSource: (source: CliMetricConfig) => request<{ source: CliMetricConfig }>(
    '/api/v1/source-types/cli.jmespath/sources',
    { method: 'POST', body: JSON.stringify(source) },
  ),
  updateCliMetricSource: (source: CliMetricConfig) => request<{ source: CliMetricConfig }>(
    `/api/v1/source-types/cli.jmespath/sources/${encodeURIComponent(source.id)}`,
    { method: 'PUT', body: JSON.stringify(source) },
  ),
  deleteCliMetricSource: (id: string) => request(
    `/api/v1/source-types/cli.jmespath/sources/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  ),
  testCliMetricConfig: (config: CliMetricConfig) => request<{ preview: CliMetricPreview }>(
    '/api/v1/source-types/cli.jmespath/test',
    { method: 'POST', body: JSON.stringify(config) },
  ),
  getHttpMetricSources: () => request<{ sources: HttpMetricConfig[] }>(
    '/api/v1/source-types/http.jmespath/sources',
  ),
  createHttpMetricSource: (source: HttpMetricConfig) => request<{ source: HttpMetricConfig }>(
    '/api/v1/source-types/http.jmespath/sources',
    { method: 'POST', body: JSON.stringify(source) },
  ),
  updateHttpMetricSource: (source: HttpMetricConfig) => request<{ source: HttpMetricConfig }>(
    `/api/v1/source-types/http.jmespath/sources/${encodeURIComponent(source.id)}`,
    { method: 'PUT', body: JSON.stringify(source) },
  ),
  deleteHttpMetricSource: (id: string) => request(
    `/api/v1/source-types/http.jmespath/sources/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  ),
  testHttpMetricConfig: (config: HttpMetricConfig) => request<{ preview: HttpMetricPreview }>(
    '/api/v1/source-types/http.jmespath/test',
    { method: 'POST', body: JSON.stringify(config) },
  ),
  getBalanceSources: () => request<{ sources: BalanceConfig[] }>(
    '/api/v1/source-types/platform.balance/sources',
  ),
  createBalanceSource: (source: BalanceConfig) => request<{ source: BalanceConfig }>(
    '/api/v1/source-types/platform.balance/sources',
    { method: 'POST', body: JSON.stringify(source) },
  ),
  updateBalanceSource: (source: BalanceConfig) => request<{ source: BalanceConfig }>(
    `/api/v1/source-types/platform.balance/sources/${encodeURIComponent(source.id)}`,
    { method: 'PUT', body: JSON.stringify(source) },
  ),
  deleteBalanceSource: (id: string) => request(
    `/api/v1/source-types/platform.balance/sources/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  ),
  testBalanceConfig: (config: BalanceConfig) => request<{ preview: HttpMetricPreview }>(
    '/api/v1/source-types/platform.balance/test',
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
  putPagePreset: (preset: PagePreset) => request<{ preset: PagePreset }>(
    `/api/v1/page-presets/${encodeURIComponent(preset.id)}`,
    { method: 'PUT', body: JSON.stringify({ title: preset.title, page: preset.page }) },
  ),
  deletePagePreset: (id: string) => request(`/api/v1/page-presets/${encodeURIComponent(id)}`, { method: 'DELETE' }),
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
