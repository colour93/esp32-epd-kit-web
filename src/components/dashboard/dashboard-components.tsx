import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { CircleAlert } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Field, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'

const PHASE_LABELS: Record<string, string> = {
  connected: '已连接',
  connecting: '连接中',
  scanning: '扫描中',
  disconnected: '已断开',
  handshaking: '初始化中',
  reconnecting: '正在重连',
  disconnecting: '断开中',
  idle: '待命',
  unavailable: '不可用',
  paused: '已暂停',
  ready: '正常',
  starting: '启动中',
  missing: '未安装',
  syncing: '同步中',
  unconfigured: '未配置',
  disabled: '已停用',
  auth_required: '未登录',
  degraded: '异常',
}

const phaseVariant = (phase: string): 'default' | 'secondary' | 'outline' | 'destructive' => {
  if (phase === 'connected' || phase === 'ready') return 'default'
  if (phase === 'paused' || phase === 'idle' || phase === 'disconnected' || phase === 'disabled') return 'secondary'
  if (phase === 'connecting' || phase === 'scanning' || phase === 'syncing' || phase === 'starting') return 'outline'
  return 'destructive'
}

export const StatusBadge = ({ phase }: { phase: string }) => (
  <Badge variant={phaseVariant(phase)}>{PHASE_LABELS[phase] ?? phase}</Badge>
)

export const SectionHeader = ({ title, action }: { title: string; action?: ReactNode }) => (
  <div className="flex min-h-12 items-center justify-between gap-4 border-b px-4 py-3">
    <h2 className="text-sm font-medium">{title}</h2>
    {action}
  </div>
)

export const MetricCard = ({ label, value, icon: Icon }: {
  label: string
  value: string
  icon: LucideIcon
}) => (
  <Card>
    <CardHeader className="flex-row items-center justify-between gap-3 pb-2">
      <CardTitle className="text-sm font-medium text-muted-foreground">{label}</CardTitle>
      <Icon className="size-4 text-muted-foreground" />
    </CardHeader>
    <CardContent>
      <div className="font-mono text-2xl font-semibold tracking-tight">{value}</div>
    </CardContent>
  </Card>
)

export const DashboardEmpty = ({ children }: { children: ReactNode }) => (
  <Empty className="min-h-32 border-0">
    <EmptyHeader>
      <EmptyMedia variant="icon"><CircleAlert /></EmptyMedia>
      <EmptyTitle>{children}</EmptyTitle>
    </EmptyHeader>
  </Empty>
)

export const NumberField = ({ label, value, min, max, onChange, disabled }: {
  label: string
  value: number
  min: number
  max: number
  onChange: (value: number) => void
  disabled?: boolean
}) => (
  <Field data-disabled={disabled || undefined}>
    <FieldLabel>{label}</FieldLabel>
    <Input
      type="number"
      min={min}
      max={max}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(Number(event.target.value))}
    />
  </Field>
)
