import { describe, expect, test } from 'bun:test'
import type { CliMetricConfig, HttpMetricConfig } from '../src/lib/agent'
import { createSourceTransferFile, parseSourceTransferFile } from '../src/lib/source-transfer'

const cli: CliMetricConfig = {
  id: 'weather',
  enabled: true,
  title: 'Weather',
  command: 'weather --json',
  items: [{
    label: 'Temp',
    data_expression: 'temp',
    description_expression: '',
    progress_expression: '',
    format: 'text',
  }],
}

const http: HttpMetricConfig = {
  id: 'balance',
  enabled: true,
  title: 'Balance',
  preset: 'custom',
  interval_sec: 300,
  timeout_ms: 5000,
  method: 'GET',
  url: 'https://example.com/balance',
  network_access: 'public',
  headers: [],
  body: '',
  auth: { type: 'bearer', secret: 'must-not-leak', secret_configured: true },
  items: [{
    label: 'Balance',
    data_expression: 'balance',
    description_expression: '',
    progress_expression: '',
    format: 'text',
  }],
}

describe('source transfer files', () => {
  test('round trips supported sources without HTTP credentials', () => {
    const exported = createSourceTransferFile([cli], [http])
    const text = JSON.stringify(exported)
    const parsed = parseSourceTransferFile(text)

    expect(parsed.cli).toEqual([cli])
    expect(parsed.http[0]?.auth).toEqual({ type: 'bearer', header_name: undefined })
    expect(text).not.toContain('must-not-leak')
    expect(text).not.toContain('secret_configured')
  })

  test('rejects duplicate and reserved IDs before import', () => {
    const duplicate = createSourceTransferFile([cli, cli], [])
    expect(() => parseSourceTransferFile(JSON.stringify(duplicate))).toThrow('数据源 ID 重复')

    const reserved = createSourceTransferFile([{ ...cli, id: 'codex' }], [])
    expect(() => parseSourceTransferFile(JSON.stringify(reserved))).toThrow('内置实例保留')
  })

  test('rejects unknown formats and versions', () => {
    expect(() => parseSourceTransferFile('{}')).toThrow('不是 EPD Agent 数据源文件')
    expect(() => parseSourceTransferFile(JSON.stringify({
      ...createSourceTransferFile([], []),
      version: 2,
    }))).toThrow('不支持的数据源文件版本')
  })
})
