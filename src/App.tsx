import { useEffect, useEffectEvent, useMemo, useRef, useState } from 'react'
import { KeyRound, LoaderCircle } from 'lucide-react'
import { Toaster, toast } from 'sonner'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Field, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { DashboardShell, type View } from '@/components/dashboard/dashboard-shell'
import { OverviewView } from '@/components/views/overview-view'
import { HardwareView } from '@/components/views/hardware-view'
import { ResourcesView } from '@/components/views/resources-view'
import { SourcesView } from '@/components/views/sources-view'
import { SecurityView } from '@/components/views/security-view'
import { DiagnosticsView, type LogLevel } from '@/components/views/diagnostics-view'
import {
  AgentApiError,
  agentApi,
  establishSession,
  subscribeSnapshots,
  type BleCandidate,
  type DeviceConfig,
  type JsonObject,
  type PageCapability,
  type PageBinding,
  type PageSlotCapability,
  type PageWidgetCapability,
  type ResourceSummary,
  type Snapshot,
} from '@/lib/agent'

const errorText = (error: unknown) => {
  if (error instanceof AgentApiError) return error.message
  return error instanceof Error ? error.message : String(error)
}

const formatTime = (seconds?: number) => {
  if (!seconds) return '—'
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit',
  }).format(new Date(seconds * 1000))
}

interface WindowLimit { usedPercent?: number; windowDurationMins?: number; resetsAt?: number }
interface LimitBucket {
  limitId?: string; limitName?: string; planType?: string
  primary?: WindowLimit | null; secondary?: WindowLimit | null
  rateLimitReachedType?: string | null
}

const isJsonObject = (value: unknown): value is JsonObject => (
  typeof value === 'object' && value !== null && !Array.isArray(value)
)

const getCodexBucket = (snapshot: Snapshot | null): LimitBucket | null => {
  const details = snapshot?.sources.find((source) => source.id === 'codex')?.details
  if (!isJsonObject(details)) return null
  const raw = details.rate_limits
  if (!isJsonObject(raw)) return null
  const byId = raw.rateLimitsByLimitId
  const selected = isJsonObject(byId) ? byId.codex : undefined
  if (isJsonObject(selected)) return selected as LimitBucket
  return isJsonObject(raw.rateLimits) ? raw.rateLimits as LimitBucket : null
}

const slotWidgets = (slot?: PageSlotCapability, pageId?: string): PageWidgetCapability[] => {
  if (!slot || slot.status !== 'active') return []
  if (slot.widgets?.length) return slot.widgets
  if (slot.schema_id && slot.schema_version) return [{
    id: slot.schema_id === 'codex.rate_limits'
      ? (pageId === 'codex.usage' ? 'codex.usage.full' : 'codex.usage.compact')
      : `${slot.schema_id}.default`,
    title: slot.id,
    schema_id: slot.schema_id, schema_version: slot.schema_version,
  }]
  return []
}

const compatibleResources = (widget: PageWidgetCapability | undefined, resources: ResourceSummary[]) => {
  if (!widget) return []
  return resources.filter((resource) => (
    resource.schema_id === widget.schema_id && resource.schema_version === widget.schema_version
  ))
}

const defaultWidgetForResource = (widgets: PageWidgetCapability[], resource?: ResourceSummary) => {
  if (!resource) return widgets[0]
  const compatible = widgets.filter((widget) => (
    widget.schema_id === resource.schema_id && widget.schema_version === resource.schema_version
  ))
  return resource.schema_id === 'generic.metrics'
    ? compatible.find((widget) => widget.id === 'generic.metric.value.1') ?? compatible[0]
    : compatible[0]
}

const normalizedBinding = (binding?: PageBinding | string): PageBinding => {
  return typeof binding === 'string'
    ? { widget_id: '', resource_key: binding }
    : { widget_id: binding?.widget_id ?? '', resource_key: binding?.resource_key ?? '' }
}

const serializedBindings = (page: PageCapability | undefined, bindings: Record<string, PageBinding>) => {
  const widgetAware = page?.slots.some((slot) => Boolean(slot.widgets?.length)) ?? false
  return Object.fromEntries(Object.entries(bindings)
    .filter(([, binding]) => binding.widget_id && binding.resource_key)
    .map(([slotId, binding]) => [slotId, widgetAware ? binding : binding.resource_key]))
}

const RESOURCE_TEMPLATE = JSON.stringify({
  key: 'example/default',
  schema_id: 'generic.metrics',
  schema_version: 1,
  revision: 1,
  updated_at: Math.floor(Date.now() / 1000),
  ttl_sec: 600,
  persistence: 'snapshot',
  payload: {
    source_status: 'ok',
    title: '示例数据',
    items: [{ label: '完成度', data: 64, description: '今日', progress: 64, format: 'percent' }],
  },
}, null, 2)

const App = () => {
  const [view, setView] = useState<View>('overview')
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [configDraft, setConfigDraft] = useState<DeviceConfig | null>(null)
  const [bootError, setBootError] = useState<string | null>(null)
  const [streamDown, setStreamDown] = useState(false)
  const [operation, setOperation] = useState<string | null>(null)
  const [pageId, setPageId] = useState('')
  const [pageBindings, setPageBindings] = useState<Record<string, PageBinding>>({})
  const [resourceEditor, setResourceEditor] = useState(RESOURCE_TEMPLATE)
  const [resourceDetail, setResourceDetail] = useState<JsonObject | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [enrollmentUntil, setEnrollmentUntil] = useState(0)
  const [resetOpen, setResetOpen] = useState(false)
  const [resetCode, setResetCode] = useState('')
  const [pairingPin, setPairingPin] = useState('')
  const [logLevel, setLogLevel] = useState<LogLevel>('all')
  const [logScope, setLogScope] = useState('all')
  const [followLogs, setFollowLogs] = useState(true)
  const logTableRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let unsubscribe = () => {}
    let cancelled = false
    const boot = async () => {
      try {
        await establishSession()
        const initial = await agentApi.snapshot()
        if (cancelled) return
        setSnapshot(initial)
        unsubscribe = subscribeSnapshots((next) => {
          setSnapshot(next)
          setStreamDown(false)
        }, () => setStreamDown(true))
      } catch (error) {
        if (!cancelled) setBootError(errorText(error))
      }
    }
    void boot()
    return () => { cancelled = true; unsubscribe() }
  }, [])

  const config = snapshot?.device.config
  const pairing = snapshot?.device.pairing
  const configRevision = config?.revision
  useEffect(() => {
    setPairingPin('')
  }, [pairing?.request_id])

  const synchronizeConfigDraft = useEffectEvent(() => {
    if (!config) return
    setConfigDraft(structuredClone(config))
    setPageId(config.page.id)
    const page = snapshot?.device.capabilities?.pages?.find((item) => item.id === config.page.id)
    setPageBindings(Object.fromEntries(Object.entries(config.page.bindings).map(([slotId, raw]) => {
      const binding = normalizedBinding(raw)
      const slot = page?.slots.find((item) => item.id === slotId)
      const widgets = slotWidgets(slot, page?.id)
      const resource = snapshot?.device.resources.find((item) => item.key === binding.resource_key)
      const configuredWidget = widgets.find((widget) => widget.id === binding.widget_id && (
        !resource || (widget.schema_id === resource.schema_id && widget.schema_version === resource.schema_version)
      ))
      return [slotId, {
        widget_id: configuredWidget?.id ?? defaultWidgetForResource(widgets, resource)?.id ?? '',
        resource_key: binding.resource_key,
      }]
    })))
  })

  useEffect(() => {
    synchronizeConfigDraft()
  }, [configRevision])

  const perform = async (key: string, action: () => Promise<unknown>, success: string, refresh = false) => {
    if (operation) return false
    setOperation(key)
    try {
      await action()
      if (refresh) setSnapshot(await agentApi.snapshot())
      toast.success(success)
      return true
    } catch (error) {
      toast.error(errorText(error))
      return false
    } finally {
      setOperation(null)
    }
  }

  const bucket = getCodexBucket(snapshot)
  const connected = snapshot?.device.phase === 'connected'
  const owner = snapshot?.device.role === 'owner'
  const pages = snapshot?.device.capabilities?.pages ?? []
  const resources = snapshot?.device.resources ?? []
  const selectedPage = pages.find((item) => item.id === pageId)
  const requiredBindingsReady = selectedPage?.slots.every((slot) => (
    slot.status !== 'active' || !slot.required || Boolean(
      pageBindings[slot.id]?.widget_id && pageBindings[slot.id]?.resource_key,
    )
  )) ?? false
  const logScopes = useMemo(
    () => Array.from(new Set((snapshot?.logs ?? []).map((entry) => entry.scope))).sort(),
    [snapshot?.logs],
  )
  const visibleLogs = useMemo(
    () => (snapshot?.logs ?? []).filter((entry) => (
      (logLevel === 'all' || entry.level === logLevel) &&
      (logScope === 'all' || entry.scope === logScope)
    )),
    [snapshot?.logs, logLevel, logScope],
  )

  const latestLogAt = snapshot?.logs[0]?.at
  const latestLogMessage = snapshot?.logs[0]?.message

  useEffect(() => {
    if (followLogs) logTableRef.current?.scrollTo({ top: 0, behavior: 'smooth' })
  }, [latestLogAt, latestLogMessage, followLogs])

  const saveConfiguration = async () => {
    if (!configDraft) return
    const before = snapshot?.device.config
    const changedHardware = before && (
      before.hardware.battery.enabled !== configDraft.hardware.battery.enabled ||
      before.hardware.io12.mode !== configDraft.hardware.io12.mode ||
      before.power.profile !== configDraft.power.profile
    )
    const ok = await perform('config', () => agentApi.patchConfig({
      device: configDraft.device,
      hardware: configDraft.hardware,
      power: configDraft.power,
      display: configDraft.display,
    } as unknown as JsonObject), '配置已提交', true)
    if (ok && changedHardware) toast.info('硬件与功耗配置将在设备重启后生效')
  }

  const inspectResource = async (resource: ResourceSummary) => {
    if (!await perform(`inspect:${resource.key}`, async () => {
      const response = await agentApi.getResource(resource.key)
      setResourceDetail(response.result.resource)
      setDetailOpen(true)
    }, '资源已读取')) return
  }

  const editResource = async (resource: ResourceSummary) => {
    await perform(`edit:${resource.key}`, async () => {
      const response = await agentApi.getResource(resource.key)
      setResourceEditor(JSON.stringify(response.result.resource, null, 2))
    }, '资源已载入编辑器')
  }

  const choosePage = (id: string) => {
    const capability = pages.find((item) => item.id === id)
    const bindings: Record<string, PageBinding> = {}
    for (const [slotIndex, slot] of (capability?.slots ?? []).entries()) {
      if (slot.status !== 'active') continue
      const widgets = slotWidgets(slot, capability?.id)
      const current = normalizedBinding(config?.page.bindings[slot.id])
      const layoutDefault = id === 'home.three' ? `generic.metric.value.${slotIndex + 1}` : undefined
      const widget = widgets.find((item) => item.id === current.widget_id)
        ?? widgets.find((item) => item.id === layoutDefault)
        ?? widgets[0]
      const compatible = compatibleResources(widget, resources)
      const autoBind = slot.required || id === 'home.three' || (id === 'home' && slot.id === 'primary')
      bindings[slot.id] = {
        widget_id: widget?.id ?? '',
        resource_key: compatible.some((resource) => resource.key === current.resource_key)
          ? current.resource_key
          : autoBind ? compatible[0]?.key ?? '' : '',
      }
    }
    setPageId(id)
    setPageBindings(bindings)
  }

  const publishEditedResource = async () => {
    let resource: JsonObject
    try {
      resource = JSON.parse(resourceEditor) as JsonObject
    } catch {
      toast.error('资源 JSON 格式无效')
      return
    }
    await perform('resource.put', () => agentApi.putResource(resource), '资源已发布', true)
  }

  const openEnrollment = async () => {
    await perform('enrollment', async () => {
      const response = await agentApi.setEnrollment(true)
      setEnrollmentUntil(Date.now() + (response.result.expires_in_sec ?? 120) * 1000)
    }, 'Enrollment 已开放')
  }

  const prepareReset = async () => {
    await perform('reset.prepare', async () => {
      await agentApi.prepareFactoryReset()
      setResetCode('')
      setResetOpen(true)
    }, '确认码已显示在设备上')
  }

  const scanDevices = () => {
    void perform('ble.scan', agentApi.scanDevices, '设备扫描已开始')
  }

  const connectDevice = (candidate: BleCandidate) => {
    void perform(
      `ble.connect:${candidate.id}`,
      () => agentApi.connectDevice(candidate.id),
      `正在连接 ${candidate.name}`,
    )
  }

  const disconnectDevice = () => {
    void perform('ble.disconnect', agentApi.disconnectDevice, '设备连接已停止')
  }

  const autoConnectDevice = () => {
    void perform('ble.auto', agentApi.autoConnectDevice, '自动连接已启动')
  }

  const submitPairingPin = () => {
    if (!pairing || pairingPin.length !== 6) return
    void perform(
      'ble.pairing.submit',
      () => agentApi.submitPairingPin(pairing.request_id, pairingPin),
      '配对码已提交，正在完成安全连接',
    )
  }

  const cancelPairing = () => {
    if (!pairing || operation) return
    setPairingPin('')
    void perform(
      'ble.pairing.cancel',
      () => agentApi.cancelPairing(pairing.request_id),
      '蓝牙配对已取消',
    )
  }

  if (bootError) {
    return (
      <main className="grid min-h-screen place-items-center bg-muted/30 p-4">
        <Card className="w-full max-w-md">
          <CardContent>
            <Alert variant="destructive">
              <AlertTitle>无法连接 Agent</AlertTitle>
              <AlertDescription>{bootError}</AlertDescription>
            </Alert>
          </CardContent>
        </Card>
      </main>
    )
  }

  if (!snapshot) {
    return (
      <main className="grid min-h-screen place-items-center bg-muted/30 p-4">
        <div className="flex w-full max-w-sm flex-col gap-3">
          <Skeleton className="h-10 w-32" />
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
        </div>
      </main>
    )
  }

  return (
    <>
      <DashboardShell
        view={view}
        onViewChange={setView}
        devicePhase={snapshot.device.phase}
        agentVersion={snapshot.agent.version}
        platform={snapshot.agent.platform}
        paused={snapshot.agent.paused}
        busy={Boolean(operation)}
        streamDown={streamDown}
        connected={connected}
        onReload={() => void perform('reload', agentApi.reloadDevice, '已刷新', true)}
        onPauseChange={() => void perform('pause', () => agentApi.setPaused(!snapshot.agent.paused), snapshot.agent.paused ? '已恢复' : '已暂停', true)}
      >
        {view === 'overview' ? (
          <OverviewView
            snapshot={snapshot}
            config={config}
            operation={operation}
            bucket={bucket}
            onScan={scanDevices}
            onConnect={connectDevice}
            onDisconnect={disconnectDevice}
            onAutoConnect={autoConnectDevice}
            onRefresh={() => void perform('refresh.auto', () => agentApi.refreshDisplay('auto'), '已刷新')}
            onFullRefresh={() => void perform('refresh.full', () => agentApi.refreshDisplay('full'), '已全刷')}
          />
        ) : null}

        {view === 'hardware' && configDraft ? (
          <HardwareView
            config={configDraft}
            setConfig={setConfigDraft}
            owner={owner}
            busy={Boolean(operation)}
            onSave={() => void saveConfiguration()}
          />
        ) : null}

        {view === 'resources' ? (
          <ResourcesView
            config={config}
            pages={pages}
            resources={resources}
            maxResources={snapshot.device.capabilities?.max_resources}
            sources={snapshot.sources}
            pageId={pageId}
            pageBindings={pageBindings}
            selectedPage={selectedPage}
            setPageBindings={setPageBindings}
            resourceEditor={resourceEditor}
            setResourceEditor={setResourceEditor}
            owner={owner}
            busy={Boolean(operation)}
            requiredBindingsReady={requiredBindingsReady}
            onChoosePage={choosePage}
            onApplyPage={() => void perform('page', () => agentApi.setPage({ id: pageId, bindings: serializedBindings(selectedPage, pageBindings) }), '已应用', true)}
            onInspect={(resource) => void inspectResource(resource)}
            onEdit={(resource) => void editResource(resource)}
            onDelete={(resource) => void perform('delete:' + resource.key, () => agentApi.deleteResource(resource.key), '已删除', true)}
            onPublish={() => void publishEditedResource()}
          />
        ) : null}

        {view === 'sources' ? <SourcesView sourceTypes={snapshot.source_types} sources={snapshot.sources} bucket={bucket} /> : null}

        {view === 'security' ? (
          <SecurityView
            bonds={snapshot.device.bonds}
            role={snapshot.device.role}
            enrollmentUntil={enrollmentUntil}
            owner={owner}
            busy={Boolean(operation)}
            onOpenEnrollment={() => void openEnrollment()}
            onCloseEnrollment={() => void perform('enrollment.close', () => agentApi.setEnrollment(false), '已关闭')}
            onTransferOwner={(id) => void perform('owner:' + id, () => agentApi.transferOwner(id), '已转移', true)}
            onRevoke={(id) => void perform('revoke:' + id, () => agentApi.revokeBond(id), '已撤销', true)}
          />
        ) : null}

        {view === 'diagnostics' ? (
          <DiagnosticsView
            agent={snapshot.agent}
            diagnostics={snapshot.device.diagnostics}
            logs={visibleLogs}
            logLevel={logLevel}
            logScope={logScope}
            logScopes={logScopes}
            followLogs={followLogs}
            streamDown={streamDown}
            owner={owner}
            busy={Boolean(operation)}
            logTableRef={logTableRef}
            onAutostartChange={(enabled) => void perform('autostart', () => agentApi.setAutostart(enabled), '已更新', true)}
            onSyncChange={(enabled) => void perform('pause', () => agentApi.setPaused(!enabled), '已更新', true)}
            onLogLevelChange={setLogLevel}
            onLogScopeChange={setLogScope}
            onFollowChange={setFollowLogs}
            onPrepareReset={() => void prepareReset()}
          />
        ) : null}
      </DashboardShell>

      <Dialog open={detailOpen} onOpenChange={setDetailOpen}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader><DialogTitle>资源</DialogTitle><DialogDescription className="sr-only">资源 JSON</DialogDescription></DialogHeader>
          <pre className="max-h-[60vh] overflow-auto rounded-lg bg-muted p-4 font-mono text-xs leading-5">{JSON.stringify(resourceDetail, null, 2)}</pre>
          <DialogFooter><DialogClose render={<Button variant="outline" />}>关闭</DialogClose></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={resetOpen} onOpenChange={setResetOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>恢复出厂</DialogTitle><DialogDescription className="sr-only">输入确认码</DialogDescription></DialogHeader>
          <Field><FieldLabel>确认码</FieldLabel><Input inputMode="numeric" maxLength={6} value={resetCode} onChange={(event) => setResetCode(event.target.value.replace(/\D/g, '').slice(0, 6))} /></Field>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />}>取消</DialogClose>
            <Button variant="destructive" disabled={resetCode.length !== 6 || Boolean(operation)} onClick={() => void perform('reset.commit', () => agentApi.commitFactoryReset(Number(resetCode)), '已恢复').then((ok) => ok && setResetOpen(false))}>确认</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={Boolean(pairing)} onOpenChange={(open) => { if (!open) cancelPairing() }}>
        <DialogContent>
          <DialogHeader><DialogTitle>蓝牙配对</DialogTitle><DialogDescription className="sr-only">{pairing?.device_name ?? 'EPD-KIT'}</DialogDescription></DialogHeader>
          <Field><FieldLabel>配对码</FieldLabel><Input autoFocus autoComplete="one-time-code" inputMode="numeric" maxLength={6} value={pairingPin} onChange={(event) => setPairingPin(event.target.value.replace(/\D/g, '').slice(0, 6))} onKeyDown={(event) => { if (event.key === 'Enter') submitPairingPin() }} /></Field>
          <div className="font-mono text-xs text-muted-foreground">{formatTime(pairing?.expires_at)}</div>
          <DialogFooter>
            <Button variant="outline" disabled={Boolean(operation)} onClick={cancelPairing}>取消</Button>
            <Button disabled={pairingPin.length !== 6 || Boolean(operation)} onClick={submitPairingPin}>
              {operation === 'ble.pairing.submit' ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <KeyRound data-icon="inline-start" />}
              配对
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Toaster position="bottom-right" richColors closeButton />
    </>
  )

}

export default App
