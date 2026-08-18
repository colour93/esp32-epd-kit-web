import { Braces, Eye, LayoutGrid, Save, Trash2 } from 'lucide-react'
import { useState, type Dispatch, type SetStateAction } from 'react'
import type { DeviceConfig, PageBinding, PageCapability, PagePreset, ResourceSummary, SourceStatus } from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Field, FieldLabel } from '@/components/ui/field'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Input } from '@/components/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { DashboardEmpty } from '@/components/dashboard/dashboard-components'
import { WidgetBindingEditor } from '@/components/resources/widget-binding-editor'

const formatAge = (seconds?: number) => {
  if (!seconds) return '从未'
  const distance = Math.max(0, Math.floor(Date.now() / 1000) - seconds)
  if (distance < 60) return `${distance} 秒`
  if (distance < 3600) return `${Math.floor(distance / 60)} 分钟`
  return `${Math.floor(distance / 3600)} 小时`
}

export const ResourcesView = ({
  config,
  pages,
  resources,
  storedResources,
  maxResources,
  sources,
  presets,
  pageId,
  pageBindings,
  selectedPage,
  setPageBindings,
  resourceEditor,
  setResourceEditor,
  owner,
  busy,
  requiredBindingsReady,
  onChoosePage,
  onChoosePreset,
  onSavePreset,
  onDeletePreset,
  onApplyPage,
  onInspect,
  onEdit,
  onDelete,
  onPublish,
}: {
  config: DeviceConfig | undefined
  pages: PageCapability[]
  resources: ResourceSummary[]
  storedResources: ResourceSummary[]
  maxResources?: number
  sources: SourceStatus[]
  presets: PagePreset[]
  pageId: string
  pageBindings: Record<string, PageBinding>
  selectedPage?: PageCapability
  setPageBindings: Dispatch<SetStateAction<Record<string, PageBinding>>>
  resourceEditor: string
  setResourceEditor: Dispatch<SetStateAction<string>>
  owner: boolean
  busy: boolean
  requiredBindingsReady: boolean
  onChoosePage: (id: string) => void
  onChoosePreset: (preset: PagePreset) => void
  onSavePreset: (title: string) => void
  onDeletePreset: (id: string) => void
  onApplyPage: () => void
  onInspect: (resource: ResourceSummary) => void
  onEdit: (resource: ResourceSummary) => void
  onDelete: (resource: ResourceSummary) => void
  onPublish: () => void
}) => {
  const [presetTitle, setPresetTitle] = useState('')
  const [presetId, setPresetId] = useState('')
  const selectedPreset = presets.find((item) => item.id === presetId)
  return (
    <>
    <Card>
      <CardHeader>
        <CardTitle>页面</CardTitle>
        <CardAction><Button size="sm" disabled={!owner || !pageId || !requiredBindingsReady || busy} onClick={onApplyPage}><Save data-icon="inline-start" />应用</Button></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 px-0">
        <div className="grid gap-4 px-4 md:grid-cols-2">
          <Field><FieldLabel>页面</FieldLabel><NativeSelect className="w-full" value={pageId.startsWith('home.') ? 'home' : pageId} onChange={(event) => onChoosePage(event.target.value)}>{pages.filter((page) => page.id !== 'home.three' && page.id !== 'home.six').map((page) => <NativeSelectOption key={page.id} value={page.id}>{page.title}</NativeSelectOption>)}</NativeSelect></Field>
          {pageId.startsWith('home') && pages.some((page) => page.id === 'home.three') ? (
            <Field><FieldLabel>布局</FieldLabel><ToggleGroup value={[pageId]} onValueChange={(value) => value[0] && onChoosePage(value[0])} variant="outline" spacing={0}><ToggleGroupItem value="home"><LayoutGrid />2</ToggleGroupItem><ToggleGroupItem value="home.three"><LayoutGrid />3</ToggleGroupItem>{pages.some((page) => page.id === 'home.six') ? <ToggleGroupItem value="home.six"><LayoutGrid />2×3</ToggleGroupItem> : null}</ToggleGroup></Field>
          ) : null}
        </div>
        <div className="grid gap-3 border-t px-4 pt-4 md:grid-cols-[1fr_1fr_auto]">
          <Field><FieldLabel>页面预设</FieldLabel><NativeSelect className="w-full" value={presetId} onChange={(event) => { const id = event.target.value; setPresetId(id); const preset = presets.find((item) => item.id === id); if (preset) { setPresetTitle(preset.title); onChoosePreset(preset) } }}><NativeSelectOption value="">选择预设</NativeSelectOption>{presets.map((preset) => <NativeSelectOption key={preset.id} value={preset.id}>{preset.title}</NativeSelectOption>)}</NativeSelect></Field>
          <Field><FieldLabel>预设名称</FieldLabel><Input value={presetTitle} placeholder="例如：工作台" onChange={(event) => setPresetTitle(event.target.value)} /></Field>
          <div className="flex items-end gap-1">
            <Button size="icon" disabled={!owner || busy || !presetTitle.trim()} title="保存为新预设" onClick={() => onSavePreset(presetTitle.trim())}><Save /><span className="sr-only">保存为新预设</span></Button>
            <Button variant="ghost" size="icon" disabled={!owner || busy || !selectedPreset} title="删除预设" onClick={() => selectedPreset && onDeletePreset(selectedPreset.id)}><Trash2 /><span className="sr-only">删除预设</span></Button>
          </div>
        </div>
        <div className="border-t">
          {selectedPage?.slots.map((slot) => slot.status === 'reserved' ? (
            <div key={slot.id} className="flex items-center justify-between border-b p-4 text-sm text-muted-foreground last:border-b-0"><span>{slot.title ?? slot.id}</span><span>保留</span></div>
          ) : (
            <WidgetBindingEditor
              key={slot.id}
              slot={slot}
              pageId={selectedPage.id}
              binding={pageBindings[slot.id] ?? { widget_id: slot.widgets?.[0]?.id ?? '', resource_key: '' }}
              resources={resources}
              sources={sources}
              onChange={(binding) => setPageBindings((current) => ({ ...current, [slot.id]: binding }))}
            />
          ))}
        </div>
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>固件资源 <span className="text-xs font-normal text-muted-foreground">{storedResources.length} / {maxResources ?? '--'}</span></CardTitle>
        <CardAction><Button variant="outline" size="sm" disabled={!owner || busy} onClick={() => setResourceEditor('')}><Braces data-icon="inline-start" />新建</Button></CardAction>
      </CardHeader>
      <CardContent className="overflow-x-auto px-0">
        {storedResources.length ? (
          <Table>
            <TableHeader><TableRow><TableHead>Key</TableHead><TableHead>Schema</TableHead><TableHead>Revision</TableHead><TableHead>更新时间</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
            <TableBody>{storedResources.map((resource) => (
              <TableRow key={resource.key}>
                <TableCell className="font-medium">{resource.key}</TableCell>
                <TableCell className="font-mono text-xs">{resource.schema_id}/v{resource.schema_version}</TableCell>
                <TableCell className="font-mono">{resource.revision}</TableCell>
                <TableCell>{formatAge(resource.updated_at)}</TableCell>
                <TableCell><div className="flex justify-end gap-1">
                  <Button variant="ghost" size="icon-sm" disabled={busy} onClick={() => onInspect(resource)}><Eye /><span className="sr-only">查看</span></Button>
                  <Button variant="ghost" size="icon-sm" disabled={!owner || busy} onClick={() => onEdit(resource)}><Braces /><span className="sr-only">编辑</span></Button>
                  <Button variant="ghost" size="icon-sm" disabled={!owner || busy || Object.values(config?.page.bindings ?? {}).some((binding) => (typeof binding === 'string' ? binding : binding.resource_key) === resource.key)} onClick={() => onDelete(resource)}><Trash2 /><span className="sr-only">删除</span></Button>
                </div></TableCell>
              </TableRow>
            ))}</TableBody>
          </Table>
        ) : <DashboardEmpty>没有资源</DashboardEmpty>}
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle>JSON</CardTitle><CardAction><Button size="sm" disabled={!owner || busy} onClick={onPublish}><Save data-icon="inline-start" />发布</Button></CardAction></CardHeader>
      <CardContent><Textarea className="min-h-80 font-mono text-xs" spellCheck={false} value={resourceEditor} onChange={(event) => setResourceEditor(event.target.value)} /></CardContent>
    </Card>
    </>
  )
}
