import type { HttpMetricConfig } from '@/lib/agent'

export const newHttpSource = (): HttpMetricConfig => ({
  id: '',
  enabled: true,
  title: 'HTTP 数据',
  interval_sec: 300,
  timeout_ms: 10_000,
  method: 'GET',
  url: '',
  network_access: 'public',
  headers: [],
  body: '',
  auth: { type: 'none', secret_configured: false },
  items: [{ label: '数据', data_expression: '', description_expression: '', progress_expression: '', format: 'text' }],
})
