import type { SourceStatus, SourceTypeStatus } from '@/lib/agent'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { DataSourceManager } from '@/components/sources/data-source-manager'

interface WindowLimit { usedPercent?: number; windowDurationMins?: number; resetsAt?: number }
interface LimitBucket { limitName?: string; primary?: WindowLimit | null; secondary?: WindowLimit | null }

const Quota = ({ title, window }: { title: string; window?: WindowLimit | null }) => {
  const value = window ? 100 - Math.max(0, Math.min(100, window.usedPercent ?? 0)) : 0
  return (
    <Card size="sm">
      <CardHeader><CardTitle>{title}</CardTitle></CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="font-mono text-3xl font-semibold">{window ? `${value}%` : '—'}</div>
        <Progress value={value} />
      </CardContent>
    </Card>
  )
}

export const SourcesView = ({ sourceTypes, sources, bucket }: {
  sourceTypes: SourceTypeStatus[]
  sources: SourceStatus[]
  bucket: LimitBucket | null
}) => (
  <>
    <DataSourceManager sourceTypes={sourceTypes} sources={sources} />
    {bucket ? (
      <Card>
        <CardHeader><CardTitle>{bucket.limitName ?? 'Codex 额度'}</CardTitle></CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <Quota title="主窗口" window={bucket.primary} />
          <Quota title="次窗口" window={bucket.secondary} />
        </CardContent>
      </Card>
    ) : null}
  </>
)
