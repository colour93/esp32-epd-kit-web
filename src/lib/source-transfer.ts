import type { CliMetricConfig, HttpMetricConfig } from '@/lib/agent'

const FILE_FORMAT = 'epd-agent-sources'
const FILE_VERSION = 1
const MAX_FILE_BYTES = 1024 * 1024
const MAX_SOURCE_COUNT = 64
const MAX_HTTP_SOURCE_COUNT = 16

type SourceTransferEntry =
  | { type: 'cli.jmespath'; config: CliMetricConfig }
  | { type: 'http.jmespath'; config: HttpMetricConfig }

export interface SourceTransferFile {
  format: typeof FILE_FORMAT
  version: typeof FILE_VERSION
  exported_at: string
  sources: SourceTransferEntry[]
}

export interface ParsedSourceTransfer {
  cli: CliMetricConfig[]
  http: HttpMetricConfig[]
}

const isRecord = (value: unknown): value is Record<string, unknown> => (
  typeof value === 'object' && value !== null && !Array.isArray(value)
)

const requireConfig = (value: unknown, type: SourceTransferEntry['type'], index: number) => {
  if (!isRecord(value)) throw new Error(`第 ${index + 1} 个数据源配置无效`)
  if (typeof value.id !== 'string' || !value.id) throw new Error(`第 ${index + 1} 个数据源缺少 ID`)
  if (typeof value.title !== 'string') throw new Error(`数据源 ${value.id} 缺少名称`)
  if (typeof value.enabled !== 'boolean') throw new Error(`数据源 ${value.id} 的启用状态无效`)
  if (!Array.isArray(value.items)) throw new Error(`数据源 ${value.id} 的指标配置无效`)
  if (!/^[a-z0-9][a-z0-9_-]{0,31}$/.test(value.id)) throw new Error(`数据源 ID 无效：${value.id}`)
  if (value.id === 'codex' || value.id === 'cc-switch') throw new Error(`数据源 ID 为内置实例保留：${value.id}`)

  if (type === 'cli.jmespath') {
    if (typeof value.command !== 'string') throw new Error(`CLI 数据源 ${value.id} 缺少命令`)
  } else {
    if (typeof value.url !== 'string' || !isRecord(value.auth)) {
      throw new Error(`HTTP 数据源 ${value.id} 配置无效`)
    }
  }
  return value
}

const portableHttpConfig = (config: HttpMetricConfig): HttpMetricConfig => ({
  ...structuredClone(config),
  auth: {
    type: config.auth.type,
    header_name: config.auth.header_name,
  },
})

export const createSourceTransferFile = (
  cli: CliMetricConfig[],
  http: HttpMetricConfig[],
): SourceTransferFile => ({
  format: FILE_FORMAT,
  version: FILE_VERSION,
  exported_at: new Date().toISOString(),
  sources: [
    ...cli.map((config): SourceTransferEntry => ({
      type: 'cli.jmespath',
      config: structuredClone(config),
    })),
    ...http.map((config): SourceTransferEntry => ({
      type: 'http.jmespath',
      config: portableHttpConfig(config),
    })),
  ],
})

export const parseSourceTransferFile = (text: string): ParsedSourceTransfer => {
  if (new Blob([text]).size > MAX_FILE_BYTES) throw new Error('导入文件不能超过 1 MiB')

  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    throw new Error('导入文件不是有效 JSON')
  }
  if (!isRecord(value) || value.format !== FILE_FORMAT) throw new Error('不是 EPD Agent 数据源文件')
  if (value.version !== FILE_VERSION) throw new Error(`不支持的数据源文件版本：${String(value.version)}`)
  if (!Array.isArray(value.sources)) throw new Error('数据源文件缺少 sources 数组')
  if (value.sources.length > MAX_SOURCE_COUNT) throw new Error(`一次最多导入 ${MAX_SOURCE_COUNT} 个数据源`)

  const parsed: ParsedSourceTransfer = { cli: [], http: [] }
  const ids = new Set<string>()
  value.sources.forEach((entry, index) => {
    if (!isRecord(entry) || (entry.type !== 'cli.jmespath' && entry.type !== 'http.jmespath')) {
      throw new Error(`第 ${index + 1} 个数据源类型无效`)
    }
    const config = requireConfig(entry.config, entry.type, index)
    const id = config.id as string
    if (ids.has(id)) throw new Error(`数据源 ID 重复：${id}`)
    ids.add(id)

    if (entry.type === 'cli.jmespath') {
      parsed.cli.push(structuredClone(config) as unknown as CliMetricConfig)
    } else {
      const http = structuredClone(config) as unknown as HttpMetricConfig
      parsed.http.push(portableHttpConfig(http))
    }
  })
  if (parsed.http.length > MAX_HTTP_SOURCE_COUNT) {
    throw new Error(`HTTP 数据源最多导入 ${MAX_HTTP_SOURCE_COUNT} 个`)
  }
  return parsed
}
