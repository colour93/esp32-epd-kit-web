import { LayoutGrid } from 'lucide-react'
import type { PageBinding } from '@/lib/agent'
import { Progress } from '@/components/ui/progress'

interface WindowLimit {
  usedPercent?: number
  windowDurationMins?: number
  resetsAt?: number
}

interface LimitBucket {
  planType?: string
  primary?: WindowLimit | null
  secondary?: WindowLimit | null
}

const remaining = (window?: WindowLimit | null) => window
  ? 100 - Math.max(0, Math.min(100, window.usedPercent ?? 0))
  : 0

const normalizedWindows = (bucket: LimitBucket | null): [WindowLimit | null, WindowLimit | null] => {
  const windows = [bucket?.primary ?? null, bucket?.secondary ?? null]
  const fiveHour = windows.find((window) => window?.windowDurationMins === 300) ?? windows[0]
  const sevenDay = windows.find((window) => window?.windowDurationMins === 10080) ?? windows[1]
  return [fiveHour, sevenDay]
}

export const HomeWidgetPreview = ({ binding, fallbackWidget, bucket }: {
  binding?: PageBinding
  fallbackWidget?: string
  bucket: LimitBucket | null
  codexPlan?: string
}) => {
  const widgetId = binding?.widget_id || fallbackWidget
  if (!binding?.resource_key || !widgetId) {
    return (
      <div className="grid min-h-28 place-items-center rounded-lg border border-dashed text-muted-foreground">
        <LayoutGrid className="size-4" />
      </div>
    )
  }

  const [fiveHour, sevenDay] = normalizedWindows(bucket)
  const values = [
    { label: '5h', value: remaining(fiveHour) },
    { label: '7d', value: remaining(sevenDay) },
  ]
  const isDual = widgetId === 'codex.usage.compact' || widgetId === 'generic.metric.dual'
  const itemIndex = Math.max(0, Number(widgetId.split('.').at(-1) ?? 1) - 1)
  const item = values[itemIndex] ?? values[0]

  return (
    <div className="flex min-h-28 flex-col gap-3 rounded-lg border bg-background p-3">
      <div className="font-mono text-xs font-medium">{binding.resource_key}</div>
      {isDual ? (
        <div className="grid flex-1 grid-cols-2 gap-3">
          {values.map((value) => (
            <div key={value.label} className="flex flex-col justify-center rounded-md bg-muted p-3 text-center">
              <span className="text-xs text-muted-foreground">{value.label}</span>
              <strong className="font-mono text-2xl">{value.value}%</strong>
            </div>
          ))}
        </div>
      ) : (
        <div className="flex flex-1 flex-col justify-center gap-2">
          <div className="flex items-end justify-between"><span className="text-xs text-muted-foreground">{item.label}</span><strong className="font-mono text-2xl">{item.value}%</strong></div>
          <Progress value={item.value} />
        </div>
      )}
    </div>
  )
}
