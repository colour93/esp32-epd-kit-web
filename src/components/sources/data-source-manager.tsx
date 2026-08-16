import { useEffect, useRef, useState } from 'react'
import { Download, FlaskConical, Globe2, LoaderCircle, Pencil, RefreshCw, Save, Terminal, Trash2, Upload, X } from 'lucide-react'
import { toast } from 'sonner'
import {
  AgentApiError,
  agentApi,
  type CliMetricConfig,
  type CliMetricPreview,
  type HttpMetricConfig,
  type SourceStatus,
  type SourceTypeStatus,
} from '@/lib/agent'
import { createSourceTransferFile, parseSourceTransferFile } from '@/lib/source-transfer'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { CliSourceEditor } from '@/components/sources/cli-source-editor'
import { HttpSourceEditor } from '@/components/sources/http-source-editor'
import { newHttpSource } from '@/components/sources/http-source-config'
import { DashboardEmpty, StatusBadge } from '@/components/dashboard/dashboard-components'

const CLI_TYPE = 'cli.jmespath'
const HTTP_TYPE = 'http.jmespath'

const errorText = (error: unknown) => error instanceof AgentApiError
  ? error.message
  : error instanceof Error ? error.message : String(error)

const formatTime = (seconds?: number) => seconds
  ? new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }).format(new Date(seconds * 1000))
  : '—'

const newCliSource = (): CliMetricConfig => ({
  id: '',
  enabled: true,
  title: 'CLI 数据',
  command: '',
  items: [{ label: '数据', data_expression: '', description_expression: '', progress_expression: '', format: 'text' }],
})

type SourceDraft =
  | { type: 'cli'; creating: boolean; config: CliMetricConfig }
  | { type: 'http'; creating: boolean; config: HttpMetricConfig }

const Preview = ({ preview }: { preview: CliMetricPreview }) => (
  <div className="grid gap-3 rounded-lg border bg-muted/30 p-3 sm:grid-cols-2 lg:grid-cols-4">
    {preview.items.map((item, index) => (
      <div key={index} className="min-w-0">
        <div className="truncate text-xs text-muted-foreground">{item.label}</div>
        <div className="mt-1 truncate font-mono text-xl font-medium">{String(item.data)}</div>
        {item.description ? <div className="truncate text-xs text-muted-foreground">{item.description}</div> : null}
      </div>
    ))}
  </div>
)

export const DataSourceManager = ({ sourceTypes, sources }: {
  sourceTypes: SourceTypeStatus[]
  sources: SourceStatus[]
}) => {
  const [cliConfigs, setCliConfigs] = useState<CliMetricConfig[]>([])
  const [httpConfigs, setHttpConfigs] = useState<HttpMetricConfig[]>([])
  const [draft, setDraft] = useState<SourceDraft | null>(null)
  const [preview, setPreview] = useState<CliMetricPreview | null>(null)
  const [busy, setBusy] = useState<string | null>('load')
  const importInput = useRef<HTMLInputElement>(null)

  const reloadConfigs = async () => {
    const [cli, http] = await Promise.all([agentApi.getCliMetricSources(), agentApi.getHttpMetricSources()])
    setCliConfigs(cli.sources)
    setHttpConfigs(http.sources)
  }

  useEffect(() => {
    let active = true
    Promise.all([agentApi.getCliMetricSources(), agentApi.getHttpMetricSources()])
      .then(([cli, http]) => {
        if (!active) return
        setCliConfigs(cli.sources)
        setHttpConfigs(http.sources)
      })
      .catch((error) => { if (active) toast.error(errorText(error)) })
      .finally(() => { if (active) setBusy(null) })
    return () => { active = false }
  }, [])

  const exportSources = () => {
    if (busy) return
    const contents = JSON.stringify(createSourceTransferFile(cliConfigs, httpConfigs), null, 2)
    const url = URL.createObjectURL(new Blob([`${contents}\n`], { type: 'application/json' }))
    const link = document.createElement('a')
    link.href = url
    link.download = `epd-agent-sources-${new Date().toISOString().slice(0, 10)}.json`
    document.body.append(link)
    link.click()
    link.remove()
    window.setTimeout(() => URL.revokeObjectURL(url), 0)
    toast.success(`已导出 ${cliConfigs.length + httpConfigs.length} 个数据源`, {
      description: 'HTTP 密钥未包含在文件中',
    })
  }

  const importSources = async (file: File) => {
    if (busy) return
    setBusy('import')
    try {
      if (file.size > 1024 * 1024) throw new Error('导入文件不能超过 1 MiB')
      const imported = parseSourceTransferFile(await file.text())
      const importedCount = imported.cli.length + imported.http.length
      if (!importedCount) throw new Error('导入文件中没有数据源')

      const cliIds = new Set(cliConfigs.map((source) => source.id))
      const httpIds = new Set(httpConfigs.map((source) => source.id))
      const collision = imported.cli.find((source) => httpIds.has(source.id))
        ?? imported.http.find((source) => cliIds.has(source.id))
      if (collision) throw new Error(`数据源 ${collision.id} 已被其他类型使用`)
      const newHttpCount = imported.http.filter((source) => !httpIds.has(source.id)).length
      if (httpConfigs.length + newHttpCount > 16) throw new Error('HTTP 数据源总数不能超过 16 个')

      const overwriteCount = imported.cli.filter((source) => cliIds.has(source.id)).length
        + imported.http.filter((source) => httpIds.has(source.id)).length
      if (overwriteCount && !window.confirm(`将覆盖 ${overwriteCount} 个同 ID 数据源，继续导入？`)) return

      for (const source of imported.cli) {
        await (cliIds.has(source.id)
          ? agentApi.updateCliMetricSource(source)
          : agentApi.createCliMetricSource(source))
      }
      for (const source of imported.http) {
        await (httpIds.has(source.id)
          ? agentApi.updateHttpMetricSource(source)
          : agentApi.createHttpMetricSource(source))
      }
      await reloadConfigs()
      setDraft(null)
      setPreview(null)
      const needsSecret = imported.http.filter((source) => source.auth.type !== 'none').length
      toast.success(`已导入 ${importedCount} 个数据源`, needsSecret ? {
        description: `${needsSecret} 个 HTTP 数据源可能需要重新填写密钥`,
      } : undefined)
    } catch (error) {
      try { await reloadConfigs() } catch { /* Preserve the original import error. */ }
      toast.error(errorText(error))
    } finally {
      setBusy(null)
      if (importInput.current) importInput.current.value = ''
    }
  }

  const beginCreate = (type: SourceDraft['type']) => {
    setPreview(null)
    setDraft(type === 'cli'
      ? { type, creating: true, config: newCliSource() }
      : { type, creating: true, config: newHttpSource() })
  }

  const beginEdit = (source: SourceStatus) => {
    setPreview(null)
    if (source.type_id === CLI_TYPE) {
      const config = cliConfigs.find((item) => item.id === source.id)
      if (config) setDraft({ type: 'cli', creating: false, config: structuredClone(config) })
      return
    }
    if (source.type_id === HTTP_TYPE) {
      const config = httpConfigs.find((item) => item.id === source.id)
      if (config) setDraft({ type: 'http', creating: false, config: structuredClone(config) })
    }
  }

  const testDraft = async () => {
    if (!draft || busy) return
    setBusy('test')
    try {
      const result = draft.type === 'cli'
        ? await agentApi.testCliMetricConfig(draft.config)
        : await agentApi.testHttpMetricConfig(draft.config)
      setPreview(result.preview)
      toast.success('测试成功')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const saveDraft = async () => {
    if (!draft || busy) return
    setBusy('save')
    try {
      if (draft.type === 'cli') {
        const result = draft.creating
          ? await agentApi.createCliMetricSource(draft.config)
          : await agentApi.updateCliMetricSource(draft.config)
        setCliConfigs((current) => draft.creating
          ? [...current, result.source]
          : current.map((source) => source.id === result.source.id ? result.source : source))
        setDraft({ type: 'cli', creating: false, config: result.source })
      } else {
        const result = draft.creating
          ? await agentApi.createHttpMetricSource(draft.config)
          : await agentApi.updateHttpMetricSource(draft.config)
        setHttpConfigs((current) => draft.creating
          ? [...current, result.source]
          : current.map((source) => source.id === result.source.id ? result.source : source))
        setDraft({ type: 'http', creating: false, config: result.source })
      }
      toast.success('已保存')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const refreshSource = async (id: string) => {
    if (busy) return
    setBusy(`refresh:${id}`)
    try {
      await agentApi.refreshSource(id)
      toast.success('已刷新')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const deleteSource = async (source: SourceStatus) => {
    if (busy || !window.confirm(`删除 ${source.id}？`)) return
    setBusy(`delete:${source.id}`)
    try {
      if (source.type_id === CLI_TYPE) {
        await agentApi.deleteCliMetricSource(source.id)
        setCliConfigs((current) => current.filter((item) => item.id !== source.id))
      } else {
        await agentApi.deleteHttpMetricSource(source.id)
        setHttpConfigs((current) => current.filter((item) => item.id !== source.id))
      }
      if (draft?.config.id === source.id) setDraft(null)
      toast.success('已删除')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const editable = (source: SourceStatus) => source.type_id === CLI_TYPE || source.type_id === HTTP_TYPE
  const typeTitle = (id: string) => sourceTypes.find((type) => type.id === id)?.title ?? id

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>数据源</CardTitle>
          <CardDescription>CLI、HTTP 与内置本地指标源</CardDescription>
          <CardAction className="flex flex-wrap justify-end gap-2">
            <input
              ref={importInput}
              className="sr-only"
              type="file"
              accept="application/json,.json"
              tabIndex={-1}
              onChange={(event) => {
                const file = event.target.files?.[0]
                if (file) void importSources(file)
              }}
            />
            <Button variant="outline" size="sm" disabled={!!busy} onClick={() => importInput.current?.click()}>
              {busy === 'import' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Upload data-icon="inline-start" />}
              导入
            </Button>
            <Button variant="outline" size="sm" disabled={!!busy} onClick={exportSources}>
              <Download data-icon="inline-start" />
              导出
            </Button>
            <Button variant="outline" size="sm" disabled={!!busy} onClick={() => beginCreate('cli')}>
              <Terminal data-icon="inline-start" />
              CLI
            </Button>
            <Button size="sm" disabled={!!busy} onClick={() => beginCreate('http')}>
              <Globe2 data-icon="inline-start" />
              HTTP
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent className="overflow-x-auto px-0">
          {sources.length ? (
            <Table>
              <TableHeader>
                <TableRow><TableHead>名称</TableHead><TableHead>类型</TableHead><TableHead>状态</TableHead><TableHead>资源</TableHead><TableHead>同步</TableHead><TableHead className="text-right">操作</TableHead></TableRow>
              </TableHeader>
              <TableBody>
                {sources.map((source) => (
                  <TableRow key={source.id}>
                    <TableCell><div className="font-medium">{source.title}</div><div className="font-mono text-xs text-muted-foreground">{source.id}</div></TableCell>
                    <TableCell><Badge variant="outline">{typeTitle(source.type_id)}</Badge></TableCell>
                    <TableCell><StatusBadge phase={source.phase} /></TableCell>
                    <TableCell className="max-w-56 truncate font-mono text-xs">{source.resource_keys.join(', ') || '—'}</TableCell>
                    <TableCell className="font-mono text-xs">{formatTime(source.last_sync_at)}</TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        <Button variant="ghost" size="icon-sm" disabled={!!busy || !source.enabled} title="刷新" onClick={() => void refreshSource(source.id)}><RefreshCw className={busy === `refresh:${source.id}` ? 'animate-spin' : undefined} /><span className="sr-only">刷新</span></Button>
                        {editable(source) ? <Button variant="ghost" size="icon-sm" disabled={!!busy} title="编辑" onClick={() => beginEdit(source)}><Pencil /><span className="sr-only">编辑</span></Button> : null}
                        {editable(source) ? <Button variant="ghost" size="icon-sm" disabled={!!busy} title="删除" onClick={() => void deleteSource(source)}><Trash2 /><span className="sr-only">删除</span></Button> : null}
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : <DashboardEmpty>没有数据源</DashboardEmpty>}
        </CardContent>
      </Card>

      {draft ? (
        <Card>
          <CardHeader>
            <CardTitle>{draft.creating ? `添加 ${draft.type === 'cli' ? 'CLI' : 'HTTP'} 数据源` : draft.config.title}</CardTitle>
            <CardDescription>{draft.type === 'cli' ? '执行本机命令并投影 JSON 输出' : '请求 JSON 接口并投影指标'}</CardDescription>
            <CardAction className="flex gap-1">
              <Button variant="ghost" size="icon-sm" disabled={!!busy} title="取消" onClick={() => setDraft(null)}><X /><span className="sr-only">取消</span></Button>
              <Button variant="outline" size="icon-sm" disabled={!!busy || !draft.config.id} title="测试" onClick={() => void testDraft()}>
                {busy === 'test' ? <LoaderCircle className="animate-spin" /> : <FlaskConical />}
                <span className="sr-only">测试</span>
              </Button>
              <Button size="icon-sm" disabled={!!busy || !draft.config.id} title="保存" onClick={() => void saveDraft()}>
                {busy === 'save' ? <LoaderCircle className="animate-spin" /> : <Save />}
                <span className="sr-only">保存</span>
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-6">
            {draft.type === 'cli'
              ? <CliSourceEditor value={draft.config} creating={draft.creating} onChange={(config) => setDraft({ ...draft, config })} />
              : <HttpSourceEditor value={draft.config} creating={draft.creating} onChange={(config) => setDraft({ ...draft, config })} />}
            {preview ? <Preview preview={preview} /> : null}
          </CardContent>
        </Card>
      ) : null}

      {sourceTypes.length ? <div className="flex flex-wrap gap-2">{sourceTypes.map((type) => <Badge key={type.id} variant="outline">{type.id}</Badge>)}</div> : null}
    </>
  )
}
