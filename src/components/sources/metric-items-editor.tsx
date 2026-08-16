import { Plus, Trash2 } from 'lucide-react'
import type { CliMetricItemConfig } from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'

const newMetricItem = (index: number): CliMetricItemConfig => ({
  label: `数据 ${index + 1}`,
  data_expression: '',
  description_expression: '',
  progress_expression: '',
  format: 'text',
})

export const MetricItemsEditor = ({
  items,
  onChange,
  locked = false,
}: {
  items: CliMetricItemConfig[]
  onChange: (items: CliMetricItemConfig[]) => void
  locked?: boolean
}) => {
  const updateItem = (index: number, update: Partial<CliMetricItemConfig>) => {
    onChange(items.map((item, itemIndex) => itemIndex === index ? { ...item, ...update } : item))
  }

  return (
    <FieldGroup className="gap-3">
      {items.map((item, index) => (
        <FieldGroup key={index} className="grid gap-3 rounded-lg border p-3 md:grid-cols-2 xl:grid-cols-[120px_minmax(160px,1fr)_minmax(160px,1fr)_minmax(160px,1fr)_120px_auto]">
          <Field>
            <FieldLabel>标签</FieldLabel>
            <Input maxLength={24} disabled={locked} value={item.label} onChange={(event) => updateItem(index, { label: event.target.value })} />
          </Field>
          <Field>
            <FieldLabel>数据</FieldLabel>
            <Input className="font-mono" disabled={locked} value={item.data_expression} onChange={(event) => updateItem(index, { data_expression: event.target.value })} />
          </Field>
          <Field>
            <FieldLabel>描述</FieldLabel>
            <Input className="font-mono" disabled={locked} value={item.description_expression} onChange={(event) => updateItem(index, { description_expression: event.target.value })} />
          </Field>
          <Field>
            <FieldLabel>进度</FieldLabel>
            <Input className="font-mono" disabled={locked} value={item.progress_expression} onChange={(event) => updateItem(index, { progress_expression: event.target.value })} />
          </Field>
          <Field>
            <FieldLabel>格式</FieldLabel>
            <NativeSelect className="w-full" disabled={locked} value={item.format} onChange={(event) => updateItem(index, { format: event.target.value as CliMetricItemConfig['format'] })}>
              <NativeSelectOption value="text">文本</NativeSelectOption>
              <NativeSelectOption value="percent">百分比</NativeSelectOption>
              <NativeSelectOption value="countdown">倒计时</NativeSelectOption>
            </NativeSelect>
          </Field>
          <Field className="justify-end">
            <FieldLabel className="sr-only">删除指标</FieldLabel>
            <Button variant="ghost" size="icon" disabled={locked || items.length === 1} onClick={() => onChange(items.filter((_, itemIndex) => itemIndex !== index))}>
              <Trash2 />
              <span className="sr-only">删除</span>
            </Button>
          </Field>
        </FieldGroup>
      ))}
      {locked ? null : (
        <Button variant="outline" size="sm" className="self-start" disabled={items.length >= 4} onClick={() => onChange([...items, newMetricItem(items.length)])}>
          <Plus data-icon="inline-start" />
          添加指标
        </Button>
      )}
    </FieldGroup>
  )
}
