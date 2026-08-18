import { useEffect, useRef, useState } from 'react'
import { CircleDollarSign, Download, ExternalLink, FlaskConical, Globe2, KeyRound, LoaderCircle, Pencil, Plus, RefreshCw, Save, Terminal, Trash2, Upload, X } from 'lucide-react'
import { toast } from 'sonner'
import {
  AgentApiError,
  agentApi,
  type BalanceConfig,
  type CliMetricConfig,
  type CliMetricPreview,
  type CodexOAuthConfig,
  type CodexOAuthStartResult,
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
import { BalanceSourceEditor } from '@/components/sources/balance-source-editor'
import { newBalanceSource } from '@/components/sources/balance-source-config'
import { DashboardEmpty, StatusBadge } from '@/components/dashboard/dashboard-components'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'

const CLI_TYPE = 'cli.jmespath'
const HTTP_TYPE = 'http.jmespath'
const BALANCE_TYPE = 'platform.balance'
const CODEX_OAUTH_TYPE = 'codex.oauth'

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
  | { type: 'balance'; creating: boolean; config: BalanceConfig }

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
  const [balanceConfigs, setBalanceConfigs] = useState<BalanceConfig[]>([])
  const [codexConfigs, setCodexConfigs] = useState<CodexOAuthConfig[]>([])
  const [codexDraft, setCodexDraft] = useState<CodexOAuthConfig | null>(null)
  const [oauthFlow, setOauthFlow] = useState<CodexOAuthStartResult | null>(null)
  const [callbackUrl, setCallbackUrl] = useState('')
  const [draft, setDraft] = useState<SourceDraft | null>(null)
  const [createDialogOpen, setCreateDialogOpen] = useState(false)
  const [preview, setPreview] = useState<CliMetricPreview | null>(null)
  const [busy, setBusy] = useState<string | null>('load')
  const importInput = useRef<HTMLInputElement>(null)

  const reloadConfigs = async () => {
    const [cli, http, balance, codex] = await Promise.all([agentApi.getCliMetricSources(), agentApi.getHttpMetricSources(), agentApi.getBalanceSources(), agentApi.getCodexOAuthSources()])
    setCliConfigs(cli.sources)
    setHttpConfigs(http.sources)
    setBalanceConfigs(balance.sources)
    setCodexConfigs(codex.sources)
  }

  useEffect(() => {
    let active = true
    Promise.all([agentApi.getCliMetricSources(), agentApi.getHttpMetricSources(), agentApi.getBalanceSources(), agentApi.getCodexOAuthSources()])
      .then(([cli, http, balance, codex]) => {
        if (!active) return
        setCliConfigs(cli.sources)
        setHttpConfigs(http.sources)
        setBalanceConfigs(balance.sources)
        setCodexConfigs(codex.sources)
      })
      .catch((error) => { if (active) toast.error(errorText(error)) })
      .finally(() => { if (active) setBusy(null) })
    return () => { active = false }
  }, [])

  const exportSources = () => {
    if (busy) return
    const contents = JSON.stringify(createSourceTransferFile(cliConfigs, httpConfigs, balanceConfigs), null, 2)
    const url = URL.createObjectURL(new Blob([`${contents}\n`], { type: 'application/json' }))
    const link = document.createElement('a')
    link.href = url
    link.download = `epd-agent-sources-${new Date().toISOString().slice(0, 10)}.json`
    document.body.append(link)
    link.click()
    link.remove()
    window.setTimeout(() => URL.revokeObjectURL(url), 0)
    toast.success(`已导出 ${cliConfigs.length + httpConfigs.length + balanceConfigs.length} 个数据源`, {
      description: 'HTTP 与平台 API Key 未包含在文件中',
    })
  }

  const importSources = async (file: File) => {
    if (busy) return
    setBusy('import')
    try {
      if (file.size > 1024 * 1024) throw new Error('导入文件不能超过 1 MiB')
      const imported = parseSourceTransferFile(await file.text())
      const importedCount = imported.cli.length + imported.http.length + imported.balance.length
      if (!importedCount) throw new Error('导入文件中没有数据源')

      const cliIds = new Set(cliConfigs.map((source) => source.id))
      const httpIds = new Set(httpConfigs.map((source) => source.id))
      const balanceIds = new Set(balanceConfigs.map((source) => source.id))
      const collision = imported.cli.find((source) => httpIds.has(source.id) || balanceIds.has(source.id))
        ?? imported.http.find((source) => cliIds.has(source.id) || balanceIds.has(source.id))
        ?? imported.balance.find((source) => cliIds.has(source.id) || httpIds.has(source.id))
      if (collision) throw new Error(`数据源 ${collision.id} 已被其他类型使用`)
      const newHttpCount = imported.http.filter((source) => !httpIds.has(source.id)).length
      if (httpConfigs.length + newHttpCount > 16) throw new Error('HTTP 数据源总数不能超过 16 个')
      const newBalanceCount = imported.balance.filter((source) => !balanceIds.has(source.id)).length
      if (balanceConfigs.length + newBalanceCount > 16) throw new Error('平台余额数据源总数不能超过 16 个')

      const overwriteCount = imported.cli.filter((source) => cliIds.has(source.id)).length
        + imported.http.filter((source) => httpIds.has(source.id)).length
        + imported.balance.filter((source) => balanceIds.has(source.id)).length
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
      for (const source of imported.balance) {
        await (balanceIds.has(source.id)
          ? agentApi.updateBalanceSource(source)
          : agentApi.createBalanceSource(source))
      }
      await reloadConfigs()
      setDraft(null)
      setPreview(null)
      const needsSecret = imported.http.filter((source) => source.auth.type !== 'none').length + imported.balance.length
      toast.success(`已导入 ${importedCount} 个数据源`, needsSecret ? {
        description: `${needsSecret} 个数据源可能需要重新填写密钥`,
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
    setCreateDialogOpen(false)
    setCodexDraft(null)
    setPreview(null)
    if (type === 'cli') setDraft({ type, creating: true, config: newCliSource() })
    else if (type === 'http') setDraft({ type, creating: true, config: newHttpSource() })
    else setDraft({ type, creating: true, config: newBalanceSource() })
  }

  const beginCodexCreate = () => {
    setCreateDialogOpen(false)
    setDraft(null)
    setPreview(null)
    setOauthFlow(null)
    setCallbackUrl('')
    setCodexDraft({
      id: '',
      enabled: true,
      title: 'Codex 账号',
      interval_sec: 60,
      authenticated: false,
    })
  }

  const beginEdit = (source: SourceStatus) => {
    setPreview(null)
    setCodexDraft(null)
    if (source.type_id === CLI_TYPE) {
      const config = cliConfigs.find((item) => item.id === source.id)
      if (config) setDraft({ type: 'cli', creating: false, config: structuredClone(config) })
      return
    }
    if (source.type_id === HTTP_TYPE) {
      const config = httpConfigs.find((item) => item.id === source.id)
      if (config) setDraft({ type: 'http', creating: false, config: structuredClone(config) })
      return
    }
    if (source.type_id === BALANCE_TYPE) {
      const config = balanceConfigs.find((item) => item.id === source.id)
      if (config) setDraft({ type: 'balance', creating: false, config: structuredClone(config) })
      return
    }
    if (source.type_id === CODEX_OAUTH_TYPE) {
      const config = codexConfigs.find((item) => item.id === source.id)
      if (config) {
        setDraft(null)
        setOauthFlow(null)
        setCallbackUrl('')
        setCodexDraft(structuredClone(config))
      }
    }
  }

  const startCodexOAuth = async () => {
    if (!codexDraft || busy) return
    setBusy('oauth-start')
    try {
      const { result } = await agentApi.startCodexOAuth(codexDraft)
      setOauthFlow(result)
      setCallbackUrl('')
      window.open(result.auth_url, '_blank', 'noopener,noreferrer')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const completeCodexOAuth = async () => {
    if (!codexDraft || !oauthFlow || !callbackUrl.trim() || busy) return
    setBusy('oauth-complete')
    try {
      const { source } = await agentApi.completeCodexOAuth(oauthFlow.session_id, callbackUrl)
      setCodexConfigs((current) => current.some((item) => item.id === source.id)
        ? current.map((item) => item.id === source.id ? source : item)
        : [...current, source])
      setCodexDraft(source)
      setOauthFlow(null)
      setCallbackUrl('')
      toast.success('Codex 账号已登录')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const saveCodexSource = async () => {
    if (!codexDraft || busy) return
    setBusy('codex-save')
    try {
      const { source } = await agentApi.updateCodexOAuthSource(codexDraft)
      setCodexConfigs((current) => current.map((item) => item.id === source.id ? source : item))
      setCodexDraft(source)
      toast.success('已保存')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const testDraft = async () => {
    if (!draft || busy) return
    setBusy('test')
    try {
      const result = draft.type === 'cli'
        ? await agentApi.testCliMetricConfig(draft.config)
        : draft.type === 'http'
          ? await agentApi.testHttpMetricConfig(draft.config)
          : await agentApi.testBalanceConfig(draft.config)
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
      } else if (draft.type === 'http') {
        const result = draft.creating
          ? await agentApi.createHttpMetricSource(draft.config)
          : await agentApi.updateHttpMetricSource(draft.config)
        setHttpConfigs((current) => draft.creating
          ? [...current, result.source]
          : current.map((source) => source.id === result.source.id ? result.source : source))
        setDraft({ type: 'http', creating: false, config: result.source })
      } else {
        const result = draft.creating
          ? await agentApi.createBalanceSource(draft.config)
          : await agentApi.updateBalanceSource(draft.config)
        setBalanceConfigs((current) => draft.creating
          ? [...current, result.source]
          : current.map((source) => source.id === result.source.id ? result.source : source))
        setDraft({ type: 'balance', creating: false, config: result.source })
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

  const updatePolicy = async (source: SourceStatus, policy: { enabled?: boolean; interval_sec?: number }) => {
    if (busy) return
    setBusy(`policy:${source.id}`)
    try {
      await agentApi.updateSourcePolicy(source.id, policy)
      toast.success(policy.enabled === false ? '已停用并清除资源' : '数据源策略已更新')
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
      } else if (source.type_id === HTTP_TYPE) {
        await agentApi.deleteHttpMetricSource(source.id)
        setHttpConfigs((current) => current.filter((item) => item.id !== source.id))
      } else if (source.type_id === BALANCE_TYPE) {
        await agentApi.deleteBalanceSource(source.id)
        setBalanceConfigs((current) => current.filter((item) => item.id !== source.id))
      } else if (source.type_id === CODEX_OAUTH_TYPE) {
        await agentApi.deleteCodexOAuthSource(source.id)
        setCodexConfigs((current) => current.filter((item) => item.id !== source.id))
      }
      if (draft?.config.id === source.id) setDraft(null)
      if (codexDraft?.id === source.id) setCodexDraft(null)
      toast.success('已删除')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  const editable = (source: SourceStatus) => source.type_id === CLI_TYPE || source.type_id === HTTP_TYPE || source.type_id === BALANCE_TYPE || source.type_id === CODEX_OAUTH_TYPE
  const typeTitle = (id: string) => sourceTypes.find((type) => type.id === id)?.title ?? id

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>数据源</CardTitle>
          <CardDescription>Codex、CLI、HTTP、平台余额与内置本地指标源</CardDescription>
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
            <Button size="sm" disabled={!!busy} onClick={() => setCreateDialogOpen(true)}>
              <Plus data-icon="inline-start" />
              新增
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent className="overflow-x-auto px-0">
          {sources.length ? (
            <Table>
              <TableHeader>
                <TableRow><TableHead>名称</TableHead><TableHead>类型</TableHead><TableHead>状态</TableHead><TableHead>启用</TableHead><TableHead>周期</TableHead><TableHead>资源</TableHead><TableHead>同步</TableHead><TableHead className="text-right">操作</TableHead></TableRow>
              </TableHeader>
              <TableBody>
                {sources.map((source) => (
                  <TableRow key={source.id}>
                    <TableCell><div className="font-medium">{source.title}</div><div className="font-mono text-xs text-muted-foreground">{source.id}</div></TableCell>
                    <TableCell><Badge variant="outline">{typeTitle(source.type_id)}</Badge></TableCell>
                    <TableCell><StatusBadge phase={source.phase} /></TableCell>
                    <TableCell><Switch checked={source.enabled} disabled={!!busy} aria-label={`${source.title} 启用状态`} onCheckedChange={(enabled) => void updatePolicy(source, { enabled })} /></TableCell>
                    <TableCell>{source.realtime ? <Badge variant="outline">被动实时</Badge> : <div className="flex items-center gap-1"><Input key={`${source.id}:${source.interval_sec}`} className="h-8 w-20 font-mono text-xs" type="number" min={10} max={86400} defaultValue={source.interval_sec ?? 60} disabled={!!busy || !source.enabled} aria-label={`${source.title} 更新周期`} onBlur={(event) => { const interval_sec = Number(event.target.value); if (interval_sec !== source.interval_sec) void updatePolicy(source, { interval_sec }) }} /><span className="text-xs text-muted-foreground">秒</span></div>}</TableCell>
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

      <Dialog open={createDialogOpen} onOpenChange={setCreateDialogOpen}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>选择数据源类型</DialogTitle>
            <DialogDescription>选择后进入对应配置。</DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Button variant="outline" className="h-auto justify-start gap-3 px-3 py-3 text-left" onClick={beginCodexCreate}>
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted"><KeyRound /></span>
              <span className="min-w-0"><span className="block font-medium">Codex OAuth</span><span className="block text-xs font-normal text-muted-foreground">独立登录并自动维护账号令牌</span></span>
            </Button>
            <Button variant="outline" className="h-auto justify-start gap-3 px-3 py-3 text-left" onClick={() => beginCreate('cli')}>
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted"><Terminal /></span>
              <span className="min-w-0"><span className="block font-medium">CLI</span><span className="block text-xs font-normal text-muted-foreground">执行本机命令并投影 JSON 输出</span></span>
            </Button>
            <Button variant="outline" className="h-auto justify-start gap-3 px-3 py-3 text-left" onClick={() => beginCreate('http')}>
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted"><Globe2 /></span>
              <span className="min-w-0"><span className="block font-medium">HTTP</span><span className="block text-xs font-normal text-muted-foreground">请求自定义 JSON 接口并投影指标</span></span>
            </Button>
            <Button variant="outline" className="h-auto justify-start gap-3 px-3 py-3 text-left" onClick={() => beginCreate('balance')}>
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted"><CircleDollarSign /></span>
              <span className="min-w-0"><span className="block font-medium">平台余额</span><span className="block text-xs font-normal text-muted-foreground">DeepSeek、Moonshot 等平台账户余额</span></span>
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {codexDraft ? (
        <Card>
          <CardHeader>
            <CardTitle>{codexDraft.authenticated ? codexDraft.title : '添加 Codex OAuth 账号'}</CardTitle>
            <CardDescription>{codexDraft.email || 'OpenAI Codex OAuth'}</CardDescription>
            <CardAction className="flex gap-1">
              <Button variant="ghost" size="icon-sm" disabled={!!busy} title="取消" onClick={() => setCodexDraft(null)}><X /><span className="sr-only">取消</span></Button>
              {codexConfigs.some((item) => item.id === codexDraft.id) ? (
                <Button variant="outline" size="icon-sm" disabled={!!busy || !codexDraft.id} title="保存" onClick={() => void saveCodexSource()}>
                  {busy === 'codex-save' ? <LoaderCircle className="animate-spin" /> : <Save />}
                  <span className="sr-only">保存</span>
                </Button>
              ) : null}
            </CardAction>
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-5">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="codex-id">数据源 ID</Label>
                <Input id="codex-id" value={codexDraft.id} disabled={codexDraft.authenticated} placeholder="codex-work" onChange={(event) => setCodexDraft({ ...codexDraft, id: event.target.value })} />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="codex-title">名称</Label>
                <Input id="codex-title" value={codexDraft.title} onChange={(event) => setCodexDraft({ ...codexDraft, title: event.target.value })} />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="codex-interval">同步间隔（秒）</Label>
                <Input id="codex-interval" type="number" min={60} max={3600} value={codexDraft.interval_sec} onChange={(event) => setCodexDraft({ ...codexDraft, interval_sec: Number(event.target.value) })} />
              </div>
              <div className="flex items-end justify-between gap-3 rounded-lg border px-3 py-2">
                <div><div className="text-sm font-medium">启用</div><div className="text-xs text-muted-foreground">{codexDraft.plan_type || 'Codex'}</div></div>
                <Switch checked={codexDraft.enabled} onCheckedChange={(enabled) => setCodexDraft({ ...codexDraft, enabled })} />
              </div>
            </div>
            {oauthFlow ? (
              <div className="flex flex-col gap-3 border-t pt-4">
                <div className="flex gap-2">
                  <Input value={callbackUrl} placeholder="http://localhost:1455/auth/callback?code=...&state=..." onChange={(event) => setCallbackUrl(event.target.value)} />
                  <Button disabled={!!busy || !callbackUrl.trim()} onClick={() => void completeCodexOAuth()}>
                    {busy === 'oauth-complete' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <KeyRound data-icon="inline-start" />}
                    完成登录
                  </Button>
                </div>
                <Button variant="outline" className="self-start" onClick={() => window.open(oauthFlow.auth_url, '_blank', 'noopener,noreferrer')}>
                  <ExternalLink data-icon="inline-start" />重新打开授权页
                </Button>
              </div>
            ) : (
              <Button className="self-start" disabled={!!busy || !codexDraft.id || !codexDraft.title} onClick={() => void startCodexOAuth()}>
                {busy === 'oauth-start' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <ExternalLink data-icon="inline-start" />}
                {codexDraft.authenticated ? '重新登录' : '登录 OpenAI'}
              </Button>
            )}
          </CardContent>
        </Card>
      ) : null}

      {draft ? (
        <Card>
          <CardHeader>
            <CardTitle>{draft.creating ? (draft.type === 'balance' ? '添加平台余额数据源' : `添加 ${draft.type === 'cli' ? 'CLI' : 'HTTP'} 数据源`) : draft.config.title}</CardTitle>
            <CardDescription>{draft.type === 'cli' ? '执行本机命令并投影 JSON 输出' : draft.type === 'http' ? '请求 JSON 接口并投影指标' : '查询 AI 平台账户余额'}</CardDescription>
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
              : draft.type === 'http'
                ? <HttpSourceEditor value={draft.config} creating={draft.creating} onChange={(config) => setDraft({ ...draft, config })} />
                : <BalanceSourceEditor value={draft.config} creating={draft.creating} onChange={(config) => setDraft({ ...draft, config })} />}
            {preview ? <Preview preview={preview} /> : null}
          </CardContent>
        </Card>
      ) : null}

      {sourceTypes.length ? <div className="flex flex-wrap gap-2">{sourceTypes.map((type) => <Badge key={type.id} variant="outline">{type.id}</Badge>)}</div> : null}
    </>
  )
}
