import { Activity, Cpu, Database, Radio, RefreshCw, RotateCcw } from 'lucide-react'
import type { BleCandidate, DeviceConfig, PageBinding, Snapshot } from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { DeviceConnectionPanel } from '@/components/device/device-connection-panel'
import { DashboardEmpty, MetricCard } from '@/components/dashboard/dashboard-components'
import { HomeWidgetPreview } from '@/components/resources/home-widget-preview'

interface WindowLimit { usedPercent?: number; windowDurationMins?: number; resetsAt?: number }
interface LimitBucket { planType?: string; primary?: WindowLimit | null; secondary?: WindowLimit | null }

const formatTime = (seconds?: number) => seconds
  ? new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' }).format(new Date(seconds * 1000))
  : '—'

const normalizeBinding = (binding?: PageBinding | string): PageBinding | undefined => binding
  ? typeof binding === 'string' ? { resource_key: binding, widget_id: '' } : binding
  : undefined

export const OverviewView = ({ snapshot, config, operation, bucket, onScan, onConnect, onDisconnect, onAutoConnect, onRefresh, onFullRefresh }: {
  snapshot: Snapshot
  config: DeviceConfig | undefined
  operation: string | null
  bucket: LimitBucket | null
  onScan: () => void
  onConnect: (candidate: BleCandidate) => void
  onDisconnect: () => void
  onAutoConnect: () => void
  onRefresh: () => void
  onFullRefresh: () => void
}) => (
  <>
    <DeviceConnectionPanel
      device={snapshot.device}
      operation={operation}
      onScan={onScan}
      onConnect={onConnect}
      onDisconnect={onDisconnect}
      onAutoConnect={onAutoConnect}
    />
    <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
      <MetricCard label="设备" value={snapshot.device.name ?? '—'} icon={Radio} />
      <MetricCard label="固件" value={snapshot.device.firmware ?? '—'} icon={Cpu} />
      <MetricCard label="资源" value={String(snapshot.device.resources.length)} icon={Database} />
      <MetricCard label="数据源" value={String(snapshot.sources.length)} icon={Activity} />
    </div>
    <div className="grid gap-4 xl:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>显示</CardTitle>
          <CardAction className="flex gap-2">
            <Button variant="outline" size="sm" disabled={!!operation} onClick={onRefresh}><RefreshCw data-icon="inline-start" />刷新</Button>
            <Button variant="outline" size="sm" disabled={!!operation} onClick={onFullRefresh}><RotateCcw data-icon="inline-start" />全刷</Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          <div className="grid gap-3 sm:grid-cols-2">
            {(config?.page.id.startsWith('home') ? Object.values(config.page.bindings) : []).slice(0, config?.page.id === 'home.three' ? 3 : 2).map((binding, index) => (
              <HomeWidgetPreview
                key={index}
                binding={normalizeBinding(binding)}
                bucket={bucket}
              />
            ))}
            {!config?.page.id.startsWith('home') ? <HomeWidgetPreview binding={normalizeBinding(config?.page.bindings.content)} fallbackWidget="codex.usage.full" bucket={bucket} /> : null}
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardHeader><CardTitle>活动</CardTitle></CardHeader>
        <CardContent className="px-0">
          {snapshot.logs.length ? (
            <div className="divide-y">
              {snapshot.logs.slice(0, 8).map((entry, index) => (
                <div key={`${entry.at}:${index}`} className="grid grid-cols-[64px_100px_1fr] gap-3 px-4 py-2.5 text-xs">
                  <time className="font-mono text-muted-foreground">{formatTime(entry.at)}</time>
                  <span className="truncate font-medium">{entry.scope}</span>
                  <span className="truncate text-muted-foreground">{entry.message}</span>
                </div>
              ))}
            </div>
          ) : <DashboardEmpty>没有活动</DashboardEmpty>}
        </CardContent>
      </Card>
    </div>
  </>
)
