import type { CliMetricConfig } from '@/lib/agent'
import { Field, FieldGroup, FieldLabel, FieldTitle } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { MetricItemsEditor } from '@/components/sources/metric-items-editor'

export const CliSourceEditor = ({
  value,
  creating,
  onChange,
}: {
  value: CliMetricConfig
  creating: boolean
  onChange: (value: CliMetricConfig) => void
}) => (
  <FieldGroup className="gap-6">
    <Field orientation="horizontal">
      <FieldTitle>启用</FieldTitle>
      <Switch checked={value.enabled} onCheckedChange={(enabled) => onChange({ ...value, enabled })} />
    </Field>
    <FieldGroup className="grid gap-4 md:grid-cols-2">
      <Field>
        <FieldLabel>实例 ID</FieldLabel>
        <Input maxLength={32} disabled={!creating} value={value.id} onChange={(event) => onChange({ ...value, id: event.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, '') })} />
      </Field>
      <Field>
        <FieldLabel>名称</FieldLabel>
        <Input maxLength={32} value={value.title} onChange={(event) => onChange({ ...value, title: event.target.value })} />
      </Field>
      <Field className="md:col-span-2">
        <FieldLabel>命令</FieldLabel>
        <Textarea className="min-h-24 font-mono" value={value.command} onChange={(event) => onChange({ ...value, command: event.target.value })} />
      </Field>
    </FieldGroup>
    <MetricItemsEditor items={value.items} onChange={(items) => onChange({ ...value, items })} />
  </FieldGroup>
)
