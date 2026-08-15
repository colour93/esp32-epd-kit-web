import {
  Bluetooth,
  BluetoothSearching,
  LoaderCircle,
  RefreshCw,
  ScanSearch,
  SignalHigh,
  SignalLow,
  SignalMedium,
  Unplug,
} from 'lucide-react'
import type { BleCandidate, DeviceStatus } from '@/lib/agent'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Separator } from '@/components/ui/separator'
import { StatusBadge } from '@/components/dashboard/dashboard-components'

const CONNECTION_MODE_LABELS: Record<DeviceStatus['connection_mode'], string> = {
  auto: '自动',
  scan: '扫描',
  manual: '手动',
  idle: '停止',
}

const shortDeviceId = (id?: string) => {
  if (!id) return '—'
  return id.length > 22 ? `${id.slice(0, 10)}…${id.slice(-8)}` : id
}

const formatTime = (seconds?: number) => {
  if (!seconds) return '—'
  return new Intl.DateTimeFormat('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(new Date(seconds * 1000))
}

const SignalIcon = ({ rssi }: { rssi?: number }) => {
  if (rssi === undefined || rssi < -85) return <SignalLow className="size-4" />
  if (rssi < -67) return <SignalMedium className="size-4" />
  return <SignalHigh className="size-4" />
}

const CandidateRow = ({ candidate, selected, connecting, disabled, onConnect }: {
  candidate: BleCandidate
  selected: boolean
  connecting: boolean
  disabled: boolean
  onConnect: () => void
}) => (
  <div className="grid grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-3 py-3 sm:grid-cols-[20px_minmax(0,1fr)_120px_auto]">
    <span className="text-muted-foreground"><SignalIcon rssi={candidate.rssi} /></span>
    <div className="min-w-0">
      <div className="truncate text-sm font-medium">{candidate.name}</div>
      <div className="truncate font-mono text-xs text-muted-foreground">{shortDeviceId(candidate.id)}</div>
    </div>
    <div className="hidden text-right sm:block">
      <div className="font-mono text-xs">{candidate.rssi === undefined ? '—' : `${candidate.rssi} dBm`}</div>
      <div className="text-xs text-muted-foreground">{candidate.owned ? 'Owner' : '未绑定'}</div>
    </div>
    <Button variant={selected ? 'secondary' : 'outline'} size="sm" disabled={disabled} onClick={onConnect}>
      {connecting ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Bluetooth data-icon="inline-start" />}
      {connecting ? '连接中' : selected ? '已选择' : '连接'}
    </Button>
  </div>
)

export const DeviceConnectionPanel = ({ device, operation, onScan, onConnect, onDisconnect, onAutoConnect }: {
  device: DeviceStatus
  operation: string | null
  onScan: () => void
  onConnect: (candidate: BleCandidate) => void
  onDisconnect: () => void
  onAutoConnect: () => void
}) => {
  const connected = device.phase === 'connected'
  const connecting = device.phase === 'connecting' || device.phase === 'handshaking'
  const scanning = device.phase === 'scanning'
  const busy = operation?.startsWith('ble.') ?? false

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <span className="grid size-8 place-items-center rounded-lg border bg-muted">
            {scanning ? <BluetoothSearching className="size-4" /> : <Bluetooth className="size-4" />}
          </span>
          <CardTitle>设备连接</CardTitle>
          <StatusBadge phase={device.phase} />
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" disabled={busy || connected || connecting} onClick={onScan}>
            <ScanSearch data-icon="inline-start" />
            扫描
          </Button>
          <Button variant="outline" size="sm" disabled={busy || connected} onClick={onAutoConnect}>
            <RefreshCw data-icon="inline-start" />
            自动
          </Button>
          <Button variant="ghost" size="sm" disabled={busy || device.connection_mode === 'idle'} onClick={onDisconnect}>
            <Unplug data-icon="inline-start" />
            停止
          </Button>
        </div>
      </CardHeader>
      <CardContent className="grid gap-6 lg:grid-cols-[280px_1fr]">
        <div className="flex flex-col gap-4 rounded-lg border bg-muted/30 p-4">
          <div className="min-w-0">
            <div className="truncate text-base font-medium">{device.name ?? 'EPD-KIT'}</div>
            <div className="truncate font-mono text-xs text-muted-foreground">{shortDeviceId(device.selected_device_id ?? device.preferred_device_id)}</div>
          </div>
          <Separator />
          <dl className="grid grid-cols-2 gap-4 text-sm">
            <div><dt className="text-muted-foreground">模式</dt><dd className="mt-1 font-medium">{CONNECTION_MODE_LABELS[device.connection_mode]}</dd></div>
            <div><dt className="text-muted-foreground">候选</dt><dd className="mt-1 font-mono font-medium">{device.candidates.length}</dd></div>
            <div><dt className="text-muted-foreground">广播</dt><dd className="mt-1 font-mono font-medium">{device.scan_observed}</dd></div>
            <div><dt className="text-muted-foreground">扫描</dt><dd className="mt-1 font-mono font-medium">{formatTime(device.scan_started_at)}</dd></div>
          </dl>
          {device.last_error ? <Alert variant="destructive"><AlertDescription>{device.last_error}</AlertDescription></Alert> : null}
        </div>

        <div className="min-w-0">
          {device.candidates.length ? (
            <div className="divide-y">
              {device.candidates.map((candidate) => (
                <CandidateRow
                  key={candidate.id}
                  candidate={candidate}
                  selected={candidate.id === device.selected_device_id}
                  connecting={connecting && candidate.id === device.selected_device_id}
                  disabled={busy || connected || (connecting && candidate.id === device.selected_device_id)}
                  onConnect={() => onConnect(candidate)}
                />
              ))}
            </div>
          ) : (
            <Empty className="min-h-48">
              <EmptyHeader>
                <EmptyMedia variant="icon">{scanning ? <LoaderCircle className="animate-spin" /> : <BluetoothSearching />}</EmptyMedia>
                <EmptyTitle>{scanning ? '扫描中' : '没有设备'}</EmptyTitle>
              </EmptyHeader>
            </Empty>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
