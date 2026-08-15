import { useEffect, useMemo, useState } from 'react'
import { Activity, Gauge, Hash, LayoutGrid } from 'lucide-react'
import {
  agentApi,
  type JsonObject,
  type PageBinding,
  type PageSlotCapability,
  type PageWidgetCapability,
  type ResourceSummary,
  type SourceStatus,
} from '@/lib/agent'
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'

type MetricPresentation = 'value' | 'bar' | 'ring' | 'dual'

interface MetricItemChoice {
  index: number
  label: string
  supportsProgress: boolean
}

const metricItemRequests = new Map<string, Promise<MetricItemChoice[]>>()

const slotWidgets = (slot?: PageSlotCapability, pageId?: string): PageWidgetCapability[] => {
  if (!slot || slot.status !== 'active') return []
  if (slot.widgets?.length) return slot.widgets
  if (!slot.schema_id || !slot.schema_version) return []
  return [{
    id: slot.schema_id === 'codex.rate_limits'
      ? (pageId === 'codex.usage' ? 'codex.usage.full' : 'codex.usage.compact')
      : `${slot.schema_id}.default`,
    title: slot.id,
    schema_id: slot.schema_id,
    schema_version: slot.schema_version,
  }]
}

const compatibleResources = (widgets: PageWidgetCapability[], resources: ResourceSummary[]) => resources.filter((resource) => (
  widgets.some((widget) => widget.schema_id === resource.schema_id && widget.schema_version === resource.schema_version)
))

const genericMetricWidget = (widgetId: string) => {
  const match = /^generic\.metric\.(value|bar|ring)\.(\d+)$/.exec(widgetId)
  if (match) return { presentation: match[1] as Exclude<MetricPresentation, 'dual'>, itemIndex: Number(match[2]) - 1 }
  return widgetId === 'generic.metric.dual' ? { presentation: 'dual' as const, itemIndex: 0 } : null
}

const loadMetricItems = (resourceKey: string, revision: number) => {
  const cacheKey = `${resourceKey}:${revision}`
  const cached = metricItemRequests.get(cacheKey)
  if (cached) return cached
  const request = agentApi.getResource(resourceKey).then((response) => {
    const payload = response.result.resource.payload
    const items = payload && typeof payload === 'object' && !Array.isArray(payload) ? (payload as JsonObject).items : undefined
    if (!Array.isArray(items)) return []
    return items.slice(0, 4).map((raw, index): MetricItemChoice => {
      const item = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw as JsonObject : {}
      return {
        index,
        label: typeof item.label === 'string' && item.label ? item.label : `数据 ${index + 1}`,
        supportsProgress: item.format === 'percent' || typeof item.progress === 'number',
      }
    })
  })
  metricItemRequests.set(cacheKey, request)
  return request
}

export const WidgetBindingEditor = ({ slot, pageId, binding, resources, sources, onChange }: {
  slot: PageSlotCapability
  pageId?: string
  binding: PageBinding
  resources: ResourceSummary[]
  sources: SourceStatus[]
  onChange: (binding: PageBinding) => void
}) => {
  const widgets = slotWidgets(slot, pageId)
  const availableResources = compatibleResources(widgets, resources)
  const selectedResource = availableResources.find((resource) => resource.key === binding.resource_key)
  const resourceWidgets = selectedResource
    ? widgets.filter((widget) => widget.schema_id === selectedResource.schema_id && widget.schema_version === selectedResource.schema_version)
    : []
  const generic = selectedResource?.schema_id === 'generic.metrics' && selectedResource.schema_version === 1
  const selectedWidget = resourceWidgets.find((widget) => widget.id === binding.widget_id) ?? resourceWidgets[0]
  const genericSelection = genericMetricWidget(selectedWidget?.id ?? '')
  const itemCount = Math.max(1, ...resourceWidgets.map((widget) => (genericMetricWidget(widget.id)?.itemIndex ?? 0) + 1))
  const fallbackItems = useMemo(() => Array.from({ length: itemCount }, (_, index) => ({ index, label: `数据 ${index + 1}`, supportsProgress: true })), [itemCount])
  const [metricItems, setMetricItems] = useState(fallbackItems)

  useEffect(() => {
    if (!generic || !selectedResource) {
      setMetricItems(fallbackItems)
      return
    }
    let active = true
    loadMetricItems(selectedResource.key, selectedResource.revision)
      .then((items) => { if (active && items.length) setMetricItems(items) })
      .catch(() => {})
    return () => { active = false }
  }, [generic, selectedResource, fallbackItems])

  const chooseResource = (resourceKey: string) => {
    const resource = availableResources.find((item) => item.key === resourceKey)
    const nextWidget = resourceWidgets.find((widget) => widget.schema_id === resource?.schema_id && widget.schema_version === resource.schema_version)
      ?? widgets.find((widget) => widget.schema_id === resource?.schema_id && widget.schema_version === resource?.schema_version)
    onChange({ resource_key: resource?.key ?? '', widget_id: nextWidget?.id ?? '' })
  }

  const chooseItem = (itemIndex: number) => {
    const presentation = genericSelection?.presentation === 'dual' ? 'value' : genericSelection?.presentation ?? 'value'
    const next = resourceWidgets.find((widget) => widget.id === `generic.metric.${presentation}.${itemIndex + 1}`)
      ?? resourceWidgets.find((widget) => widget.id === `generic.metric.value.${itemIndex + 1}`)
    if (next) onChange({ ...binding, widget_id: next.id })
  }

  const choosePresentation = (presentation: string) => {
    const nextId = presentation === 'dual' ? 'generic.metric.dual' : `generic.metric.${presentation}.${(genericSelection?.itemIndex ?? 0) + 1}`
    const next = resourceWidgets.find((widget) => widget.id === nextId)
    if (next) onChange({ ...binding, widget_id: next.id })
  }

  const sourceName = (resourceKey: string) => sources.find((source) => source.resource_keys.includes(resourceKey))?.title

  return (
    <div className="grid gap-4 border-b p-4 last:border-b-0 lg:grid-cols-[180px_1fr]">
      <div className="flex items-start gap-3">
        <span className="grid size-8 place-items-center rounded-lg bg-muted"><LayoutGrid className="size-4" /></span>
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{slot.title ?? slot.id}</div>
          <div className="truncate font-mono text-xs text-muted-foreground">{slot.id}</div>
        </div>
      </div>
      <FieldGroup className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        <Field>
          <FieldLabel>数据源</FieldLabel>
          <NativeSelect className="w-full" value={binding.resource_key} onChange={(event) => chooseResource(event.target.value)}>
            <NativeSelectOption value="">{slot.required ? '请选择' : '不绑定'}</NativeSelectOption>
            {availableResources.map((resource) => <NativeSelectOption key={resource.key} value={resource.key}>{sourceName(resource.key) ?? resource.key}</NativeSelectOption>)}
          </NativeSelect>
        </Field>
        {generic ? (
          <Field>
            <FieldLabel>数据项</FieldLabel>
            <NativeSelect className="w-full" value={genericSelection?.itemIndex ?? 0} disabled={genericSelection?.presentation === 'dual'} onChange={(event) => chooseItem(Number(event.target.value))}>
              {metricItems.map((item) => <NativeSelectOption key={item.index} value={item.index}>{item.label}</NativeSelectOption>)}
            </NativeSelect>
          </Field>
        ) : null}
        {generic ? (
          <Field>
            <FieldLabel>呈现</FieldLabel>
            <ToggleGroup value={[genericSelection?.presentation ?? 'value']} onValueChange={(value) => choosePresentation(value[0] ?? 'value')} variant="outline" spacing={0}>
              <ToggleGroupItem value="value" aria-label="数值"><Hash /></ToggleGroupItem>
              <ToggleGroupItem value="bar" aria-label="条形" disabled={!metricItems[genericSelection?.itemIndex ?? 0]?.supportsProgress}><Activity /></ToggleGroupItem>
              <ToggleGroupItem value="ring" aria-label="环形" disabled={!metricItems[genericSelection?.itemIndex ?? 0]?.supportsProgress}><Gauge /></ToggleGroupItem>
              <ToggleGroupItem value="dual" aria-label="双项"><LayoutGrid /></ToggleGroupItem>
            </ToggleGroup>
          </Field>
        ) : null}
      </FieldGroup>
    </div>
  )
}
