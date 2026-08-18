import { Braces, Trash2 } from 'lucide-react'
import type { RefObject } from 'react'
import type { AgentStatus, JsonObject, LogEntry } from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Field, FieldLabel, FieldTitle } from '@/components/ui/field'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { DashboardEmpty, StatusBadge } from '@/components/dashboard/dashboard-components'

export type LogLevel = 'all' | 'debug' | 'info' | 'warn' | 'error'

const formatTime = (seconds: number) => new Intl.DateTimeFormat('zh-CN', {
  hour: '2-digit', minute: '2-digit', second: '2-digit',
}).format(new Date(seconds * 1000))

export const DiagnosticsView = ({
  agent,
  diagnostics,
  logs,
  logLevel,
  logScope,
  logScopes,
  followLogs,
  streamDown,
  owner,
  busy,
  logTableRef,
  onAutostartChange,
  onSyncChange,
  onLogLevelChange,
  onLogScopeChange,
  onFollowChange,
  onPrepareReset,
}: {
  agent: AgentStatus
  diagnostics?: JsonObject
  logs: LogEntry[]
  logLevel: LogLevel
  logScope: string
  logScopes: string[]
  followLogs: boolean
  streamDown: boolean
  owner: boolean
  busy: boolean
  logTableRef: RefObject<HTMLDivElement | null>
  onAutostartChange: (enabled: boolean) => void
  onSyncChange: (enabled: boolean) => void
  onLogLevelChange: (level: LogLevel) => void
  onLogScopeChange: (scope: string) => void
  onFollowChange: (enabled: boolean) => void
  onPrepareReset: () => void
}) => (
  <>
    <Card>
      <CardHeader><CardTitle>Agent</CardTitle><CardAction><span className="font-mono text-xs text-muted-foreground">{agent.version} · {agent.platform}</span></CardAction></CardHeader>
      <CardContent className="flex flex-col gap-4">
        <Field orientation="horizontal"><FieldTitle>登录时启动</FieldTitle><Switch checked={agent.autostart_enabled} disabled={busy} onCheckedChange={onAutostartChange} /></Field>
        <Field orientation="horizontal"><FieldTitle>BLE 同步</FieldTitle><Switch checked={!agent.paused} disabled={busy} onCheckedChange={onSyncChange} /></Field>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle>设备诊断</CardTitle></CardHeader>
      <CardContent><pre className="max-h-80 overflow-auto rounded-lg bg-muted p-4 font-mono text-xs leading-5">{JSON.stringify(diagnostics ?? {}, null, 2)}</pre></CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle>日志</CardTitle><CardAction><StatusBadge phase={streamDown ? 'reconnecting' : 'ready'} /></CardAction></CardHeader>
      <CardContent className="flex flex-col gap-4 px-0">
        <div className="flex flex-wrap items-end gap-3 px-4">
          <Field className="w-auto"><FieldLabel>级别</FieldLabel><ToggleGroup value={[logLevel]} onValueChange={(value) => value[0] && onLogLevelChange(value[0] as LogLevel)} variant="outline" spacing={0}>{(['all', 'debug', 'info', 'warn', 'error'] as const).map((level) => <ToggleGroupItem key={level} value={level}>{level}</ToggleGroupItem>)}</ToggleGroup></Field>
          <Field className="w-44"><FieldLabel>Scope</FieldLabel><NativeSelect className="w-full" value={logScope} onChange={(event) => onLogScopeChange(event.target.value)}><NativeSelectOption value="all">all</NativeSelectOption>{logScopes.map((scope) => <NativeSelectOption key={scope} value={scope}>{scope}</NativeSelectOption>)}</NativeSelect></Field>
          <Field orientation="horizontal" className="ml-auto w-auto pb-1"><FieldTitle>跟随</FieldTitle><Switch checked={followLogs} onCheckedChange={onFollowChange} /></Field>
        </div>
        <div ref={logTableRef} className="max-h-96 overflow-auto border-y bg-muted/30">
          {logs.length ? logs.map((entry, index) => (
            <div key={`${entry.at}:${entry.scope}:${index}`} className="grid grid-cols-[64px_64px_100px_1fr] gap-3 border-b px-4 py-2 font-mono text-xs last:border-b-0">
              <time className="text-muted-foreground">{formatTime(entry.at)}</time>
              <span>{entry.level}</span>
              <span className="truncate font-medium">{entry.scope}</span>
              <code className="truncate text-muted-foreground">{entry.message}</code>
            </div>
          )) : <DashboardEmpty>没有日志</DashboardEmpty>}
        </div>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle className="text-destructive">恢复出厂</CardTitle><CardAction><Button variant="destructive" size="sm" disabled={!owner || busy} onClick={onPrepareReset}><Trash2 data-icon="inline-start" />恢复</Button></CardAction></CardHeader>
      <CardContent className="sr-only"><Braces />factory reset</CardContent>
    </Card>
  </>
)
