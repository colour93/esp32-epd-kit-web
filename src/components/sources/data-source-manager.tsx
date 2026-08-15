import { useEffect, useState } from 'react'
import { FlaskConical, LoaderCircle, Pencil, Plus, RefreshCw, Save, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import {
  AgentApiError,
  agentApi,
  type CliMetricConfig,
  type CliMetricPreview,
  type SourceStatus,
  type SourceTypeStatus,
} from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { DashboardEmpty, StatusBadge } from '@/components/dashboard/dashboard-components'

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

export const DataSourceManager = ({ sourceTypes, sources }: {
  sourceTypes: SourceTypeStatus[]
  sources: SourceStatus[]
}) => {
  const [configs, setConfigs] = useState<CliMetricConfig[]>([])
  const [draft, setDraft] = useState<CliMetricConfig | null>(null)
  const [creating, setCreating] = useState(false)
  const [preview, setPreview] = useState<CliMetricPreview | null>(null)
  const [busy, setBusy] = useState<string | null>('load')

  useEffect(() => {
    let active = true
    agentApi.getCliMetricSources()
      .then(({ sources: cliSources }) => { if (active) setConfigs(cliSources) })
      .catch((error) => { if (active) toast.error(errorText(error)) })
      .finally(() => { if (active) setBusy(null) })
    return () => { active = false }
  }, [])

  const beginCreate = () => {
    setCreating(true)
    setPreview(null)
    setDraft(newCliSource())
  }

  const beginEdit = (id: string) => {
    const config = configs.find((source) => source.id === id)
    if (!config) return
    setCreating(false)
    setPreview(null)
    setDraft(structuredClone(config))
  }

  const testDraft = async () => {
    if (!draft || busy) return
    setBusy('test')
    try {
      const result = await agentApi.testCliMetricConfig(draft)
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
      const result = creating
        ? await agentApi.createCliMetricSource(draft)
        : await agentApi.updateCliMetricSource(draft)
      setConfigs((current) => creating
        ? [...current, result.source]
        : current.map((source) => source.id === result.source.id ? result.source : source))
      setDraft(result.source)
      setCreating(false)
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

  const deleteSource = async (id: string) => {
    if (busy || !window.confirm(`删除 ${id}？`)) return
    setBusy(`delete:${id}`)
    try {
      await agentApi.deleteCliMetricSource(id)
      setConfigs((current) => current.filter((source) => source.id !== id))
      if (draft?.id === id) setDraft(null)
      toast.success('已删除')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>数据源</CardTitle>
          <CardAction><Button size="sm" onClick={beginCreate}><Plus data-icon="inline-start" />添加</Button></CardAction>
        </CardHeader>
        <CardContent className="overflow-x-auto px-0">
          {sources.length ? (
            <Table>
              <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>类型</TableHead><TableHead>状态</TableHead><TableHead>资源</TableHead><TableHead>同步</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
              <TableBody>
                {sources.map((source) => (
                  <TableRow key={source.id}>
                    <TableCell><div className="font-medium">{source.title}</div><div className="font-mono text-xs text-muted-foreground">{source.id}</div></TableCell>
                    <TableCell className="font-mono text-xs">{source.type_id}</TableCell>
                    <TableCell><StatusBadge phase={source.phase} /></TableCell>
                    <TableCell className="max-w-56 truncate font-mono text-xs">{source.resource_keys.join(', ') || '—'}</TableCell>
                    <TableCell className="font-mono text-xs">{formatTime(source.last_sync_at)}</TableCell>
                    <TableCell><div className="flex justify-end gap-1">
                      <Button variant="ghost" size="icon-sm" disabled={!!busy || !source.enabled} onClick={() => void refreshSource(source.id)}><RefreshCw className={busy === `refresh:${source.id}` ? 'animate-spin' : undefined} /><span className="sr-only">刷新</span></Button>
                      {source.type_id === 'cli.jmespath' ? <Button variant="ghost" size="icon-sm" disabled={!!busy} onClick={() => beginEdit(source.id)}><Pencil /><span className="sr-only">编辑</span></Button> : null}
                      {source.type_id === 'cli.jmespath' ? <Button variant="ghost" size="icon-sm" disabled={!!busy} onClick={() => void deleteSource(source.id)}><Trash2 /><span className="sr-only">删除</span></Button> : null}
                    </div></TableCell>
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
            <CardTitle>{creating ? '添加数据源' : draft.title}</CardTitle>
            <CardAction><div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={() => setDraft(null)}>取消</Button>
              <Button variant="outline" size="sm" disabled={!!busy || !draft.id} onClick={() => void testDraft()}>
                {busy === 'test' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <FlaskConical data-icon="inline-start" />}
                测试
              </Button>
              <Button size="sm" disabled={!!busy || !draft.id} onClick={() => void saveDraft()}>
                {busy === 'save' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Save data-icon="inline-start" />}
                保存
              </Button>
            </div></CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <Field orientation="horizontal">
              <FieldLabel>启用</FieldLabel>
              <Switch checked={draft.enabled} onCheckedChange={(enabled) => setDraft({ ...draft, enabled })} />
            </Field>
            <FieldGroup className="grid gap-4 md:grid-cols-2">
              <Field><FieldLabel>实例 ID</FieldLabel><Input maxLength={32} disabled={!creating} value={draft.id} onChange={(event) => setDraft({ ...draft, id: event.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, '') })} /></Field>
              <Field><FieldLabel>名称</FieldLabel><Input maxLength={32} value={draft.title} onChange={(event) => setDraft({ ...draft, title: event.target.value })} /></Field>
              <Field className="md:col-span-2"><FieldLabel>命令</FieldLabel><Textarea className="min-h-24 font-mono" value={draft.command} onChange={(event) => setDraft({ ...draft, command: event.target.value })} /></Field>
            </FieldGroup>

            <div className="flex flex-col gap-3">
              {draft.items.map((item, index) => (
                <div key={index} className="grid gap-3 rounded-lg border p-3 md:grid-cols-2 xl:grid-cols-[120px_1fr_1fr_1fr_120px_auto]">
                  <Field><FieldLabel>标签</FieldLabel><Input value={item.label} onChange={(event) => setDraft({ ...draft, items: draft.items.map((value, itemIndex) => itemIndex === index ? { ...value, label: event.target.value } : value) })} /></Field>
                  <Field><FieldLabel>数据</FieldLabel><Input value={item.data_expression} onChange={(event) => setDraft({ ...draft, items: draft.items.map((value, itemIndex) => itemIndex === index ? { ...value, data_expression: event.target.value } : value) })} /></Field>
                  <Field><FieldLabel>描述</FieldLabel><Input value={item.description_expression} onChange={(event) => setDraft({ ...draft, items: draft.items.map((value, itemIndex) => itemIndex === index ? { ...value, description_expression: event.target.value } : value) })} /></Field>
                  <Field><FieldLabel>进度</FieldLabel><Input value={item.progress_expression} onChange={(event) => setDraft({ ...draft, items: draft.items.map((value, itemIndex) => itemIndex === index ? { ...value, progress_expression: event.target.value } : value) })} /></Field>
                  <Field><FieldLabel>格式</FieldLabel><NativeSelect className="w-full" value={item.format} onChange={(event) => setDraft({ ...draft, items: draft.items.map((value, itemIndex) => itemIndex === index ? { ...value, format: event.target.value as typeof value.format } : value) })}><NativeSelectOption value="text">文本</NativeSelectOption><NativeSelectOption value="percent">百分比</NativeSelectOption><NativeSelectOption value="countdown">倒计时</NativeSelectOption></NativeSelect></Field>
                  <div className="flex items-end"><Button variant="ghost" size="icon" disabled={draft.items.length === 1} onClick={() => setDraft({ ...draft, items: draft.items.filter((_, itemIndex) => itemIndex !== index) })}><Trash2 /><span className="sr-only">删除</span></Button></div>
                </div>
              ))}
              <Button variant="outline" size="sm" className="self-start" disabled={draft.items.length >= 4} onClick={() => setDraft({ ...draft, items: [...draft.items, { label: `数据 ${draft.items.length + 1}`, data_expression: '', description_expression: '', progress_expression: '', format: 'text' }] })}><Plus data-icon="inline-start" />添加数据项</Button>
            </div>

            {preview ? (
              <div className="grid gap-3 rounded-lg border bg-muted/30 p-3 sm:grid-cols-2 lg:grid-cols-4">
                {preview.items.map((item, index) => <div key={index}><div className="text-xs text-muted-foreground">{item.label}</div><div className="mt-1 font-mono text-xl font-medium">{String(item.data)}</div></div>)}
              </div>
            ) : null}
          </CardContent>
        </Card>
      ) : null}

      {sourceTypes.length ? <div className="flex flex-wrap gap-2">{sourceTypes.map((type) => <span key={type.id} className="font-mono text-xs text-muted-foreground">{type.id}</span>)}</div> : null}
    </>
  )
}
