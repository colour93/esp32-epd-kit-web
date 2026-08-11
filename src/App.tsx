import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  Activity,
  BatteryCharging,
  Bluetooth,
  BluetoothSearching,
  Bot,
  Braces,
  ChevronRight,
  CircleAlert,
  Clock3,
  Cpu,
  Database,
  Eye,
  FlaskConical,
  Gauge,
  HardDrive,
  KeyRound,
  LayoutDashboard,
  LoaderCircle,
  ListChecks,
  MonitorCog,
  Pause,
  Play,
  Power,
  Radio,
  RefreshCw,
  RotateCcw,
  Save,
  ScanSearch,
  Settings2,
  ShieldCheck,
  SignalHigh,
  SignalLow,
  SignalMedium,
  SlidersHorizontal,
  Trash2,
  Unplug,
  Users,
  Zap,
  type LucideIcon,
} from 'lucide-react'
import { Toaster, toast } from 'sonner'
import { Button } from './components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from './components/ui/dialog'
import { Input } from './components/ui/input'
import { Switch } from './components/ui/switch'
import {
  AgentApiError,
  agentApi,
  establishSession,
  subscribeSnapshots,
  type BleCandidate,
  type DeviceConfig,
  type DeviceStatus,
  type FeishuProjectConfig,
  type FeishuProjectPreview,
  type JsonObject,
  type PageCapability,
  type ResourceSummary,
  type Snapshot,
} from './lib/agent'

type View = 'overview' | 'hardware' | 'resources' | 'producers' | 'security' | 'diagnostics'
type LogLevel = 'all' | 'info' | 'warn' | 'error'

const NAV_ITEMS: Array<{ id: View; label: string; icon: LucideIcon }> = [
  { id: 'overview', label: '概览', icon: LayoutDashboard },
  { id: 'hardware', label: '硬件与功耗', icon: SlidersHorizontal },
  { id: 'resources', label: '资源与视图', icon: Database },
  { id: 'producers', label: '数据源', icon: Bot },
  { id: 'security', label: '受信主机', icon: ShieldCheck },
  { id: 'diagnostics', label: '诊断', icon: Activity },
]

const PHASE_LABELS: Record<string, string> = {
  connected: '已连接', connecting: '连接中', scanning: '扫描中', disconnected: '已断开',
  disconnecting: '断开中', idle: '待命', unavailable: '不可用', paused: '已暂停',
  ready: '正常', starting: '启动中', missing: '未安装',
  syncing: '同步中', unconfigured: '未配置', disabled: '已停用',
  auth_required: '未登录', degraded: '同步异常',
}

const CONNECTION_MODE_LABELS: Record<DeviceStatus['connection_mode'], string> = {
  auto: '自动连接',
  scan: '手动扫描',
  manual: '指定设备',
  idle: '手动停止',
}

function errorText(error: unknown) {
  if (error instanceof AgentApiError) return error.message
  return error instanceof Error ? error.message : String(error)
}

function formatTime(seconds?: number) {
  if (!seconds) return '—'
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit',
  }).format(new Date(seconds * 1000))
}

function formatAge(seconds?: number) {
  if (!seconds) return '从未'
  const distance = Math.max(0, Math.floor(Date.now() / 1000) - seconds)
  if (distance < 60) return `${distance} 秒前`
  if (distance < 3600) return `${Math.floor(distance / 60)} 分钟前`
  return `${Math.floor(distance / 3600)} 小时前`
}

function phaseTone(phase: string) {
  if (phase === 'connected' || phase === 'ready') return 'good'
  if (phase === 'connecting' || phase === 'disconnecting' || phase === 'scanning' || phase === 'starting') return 'working'
  if (phase === 'paused' || phase === 'idle' || phase === 'disconnected') return 'muted'
  return 'bad'
}

function StatusPill({ phase }: { phase: string }) {
  return <span className={`status-pill ${phaseTone(phase)}`}><i />{PHASE_LABELS[phase] ?? phase}</span>
}

function SectionTitle({ icon: Icon, title, detail, action }: {
  icon: LucideIcon; title: string; detail?: string; action?: ReactNode
}) {
  return (
    <header className="section-title">
      <div className="section-heading">
        <span className="section-icon"><Icon /></span>
        <div><h2>{title}</h2>{detail ? <p>{detail}</p> : null}</div>
      </div>
      {action}
    </header>
  )
}

function Metric({ label, value, detail, icon: Icon, tone = 'default' }: {
  label: string; value: string; detail: string; icon: LucideIcon; tone?: string
}) {
  return (
    <article className={`metric ${tone}`}>
      <div className="metric-label"><Icon />{label}</div>
      <strong>{value}</strong>
      <span>{detail}</span>
    </article>
  )
}

function EmptyState({ children }: { children: ReactNode }) {
  return <div className="empty-state"><CircleAlert /> <span>{children}</span></div>
}

function shortDeviceId(id?: string) {
  if (!id) return '—'
  return id.length > 18 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id
}

function connectionErrorText(error: string) {
  if (error.includes('Bluetooth access unavailable') || error.includes('Permission denied')) {
    return '蓝牙权限不可用，请在「系统设置 > 隐私与安全性 > 蓝牙」中允许 EPD Agent。'
  }
  if (error.includes('Peer removed pairing information')) {
    return '设备已清除旧配对信息，Agent 已刷新 macOS 蓝牙状态并重试。设备显示配对码时请完成确认。'
  }
  if (error.includes('BLE connection timed out')) {
    return '连接超过 12 秒未完成，本轮已结束，可停止或重新选择设备。'
  }
  if (error.includes('selected EPD-KIT device') && error.includes('was not found')) {
    return '所选设备已离开广播范围，请重新扫描。'
  }
  if (error.includes('no EPD-KIT BLE v4 device found')) {
    return '本轮扫描未发现 EPD-KIT 设备，请确认设备供电；若刚发生配对错误，请重启设备后重新扫描。'
  }
  return error
}

function SignalIcon({ rssi }: { rssi?: number }) {
  if (rssi === undefined || rssi < -85) return <SignalLow />
  if (rssi < -67) return <SignalMedium />
  return <SignalHigh />
}

function CandidateRow({ candidate, selected, connecting, disabled, onConnect }: {
  candidate: BleCandidate
  selected: boolean
  connecting: boolean
  disabled: boolean
  onConnect: () => void
}) {
  return (
    <div className={`candidate-row ${selected ? 'selected' : ''}`}>
      <div className="candidate-signal"><SignalIcon rssi={candidate.rssi} /></div>
      <div className="candidate-identity">
        <b>{candidate.name}</b>
        <span>{shortDeviceId(candidate.id)}</span>
      </div>
      <div className="candidate-meta">
        <b>{candidate.rssi === undefined ? '—' : `${candidate.rssi} dBm`}</b>
        <span>{candidate.owned === true ? '已有 Owner' : candidate.owned === false ? '待设置 Owner' : candidate.advertises_service ? 'v4 service' : 'name match'}</span>
      </div>
      <Button variant={selected ? 'signal' : 'outline'} size="sm" disabled={disabled} onClick={onConnect}>
        {connecting ? <LoaderCircle className="spin" /> : <Bluetooth />}
        {connecting ? '连接中' : selected ? '已选择' : '连接'}
      </Button>
    </div>
  )
}

function DeviceConnectionPanel({ device, operation, onScan, onConnect, onDisconnect, onAutoConnect }: {
  device: DeviceStatus
  operation: string | null
  onScan: () => void
  onConnect: (candidate: BleCandidate) => void
  onDisconnect: () => void
  onAutoConnect: () => void
}) {
  const connected = device.phase === 'connected'
  const connecting = device.phase === 'connecting'
  const scanning = device.phase === 'scanning'
  const active = device.connection_mode !== 'idle'
  const busy = operation?.startsWith('ble.') ?? false
  const mode = CONNECTION_MODE_LABELS[device.connection_mode] ?? device.connection_mode

  return (
    <section className={`connection-console ${connected ? 'connected' : ''}`}>
      <header className="connection-header">
        <div className="connection-title">
          <span className="connection-icon">
            {scanning ? <BluetoothSearching className="scan-pulse" /> : <Bluetooth />}
          </span>
          <div>
            <span>BLE CONNECTION</span>
            <h2>设备连接</h2>
          </div>
          <StatusPill phase={device.phase} />
        </div>
        <div className="connection-actions">
          <Button className="connection-action" variant="outline" size="sm" disabled={busy || connected || connecting} onClick={onScan}>
            <ScanSearch className={scanning ? 'spin' : ''} />{scanning ? '扫描中' : '扫描'}
          </Button>
          <Button className="connection-action" variant="outline" size="sm" disabled={busy || connected || (scanning && device.connection_mode === 'auto')} onClick={onAutoConnect}>
            <RotateCcw />自动连接
          </Button>
          <Button className="connection-stop" variant="ghost" size="sm" disabled={busy || !active} onClick={onDisconnect}>
            <Unplug />停止
          </Button>
        </div>
      </header>

      <div className="connection-body">
        <div className="connection-current">
          <div className="connection-current-head">
            <span className={`connection-beacon ${connected ? 'online' : scanning || connecting ? 'working' : ''}`}><i /></span>
            <div>
              <span>{connected ? 'CURRENT DEVICE' : 'CONNECTION STATE'}</span>
              <strong>{connected ? device.name ?? 'EPD-KIT' : PHASE_LABELS[device.phase] ?? device.phase}</strong>
            </div>
          </div>
          <dl className="connection-facts">
            <div><dt>模式</dt><dd>{mode}</dd></div>
            <div><dt>目标</dt><dd title={device.selected_device_id ?? device.preferred_device_id}>{shortDeviceId(device.selected_device_id ?? device.preferred_device_id)}</dd></div>
            <div><dt>广播</dt><dd>{device.scan_observed}</dd></div>
            <div><dt>候选</dt><dd>{device.candidates.length}</dd></div>
          </dl>
          {device.last_error ? <div className="connection-error"><CircleAlert /><span>{connectionErrorText(device.last_error)}</span></div> : null}
        </div>

        <div className="candidate-list">
          <div className="candidate-list-head">
            <div><b>可连接设备</b><span>{scanning ? '实时更新' : `最近扫描 ${formatTime(device.scan_started_at)}`}</span></div>
            <span>{device.candidates.length.toString().padStart(2, '0')}</span>
          </div>
          <div className="candidate-scroll">
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
            {!device.candidates.length ? (
              <div className="candidate-empty">
                {scanning ? <LoaderCircle className="spin" /> : <BluetoothSearching />}
                <div><b>{scanning ? '正在监听 EPD-KIT 广播' : '未发现可连接设备'}</b><span>已观察 {device.scan_observed} 个 BLE 广播</span></div>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </section>
  )
}

interface WindowLimit { usedPercent?: number; windowDurationMins?: number; resetsAt?: number }
interface LimitBucket {
  limitId?: string; limitName?: string; planType?: string
  primary?: WindowLimit | null; secondary?: WindowLimit | null
  rateLimitReachedType?: string | null
}

function getCodexBucket(snapshot: Snapshot | null): LimitBucket | null {
  const raw = snapshot?.producers.find((producer) => producer.id === 'codex.usage')?.details.rate_limits as JsonObject | undefined
  if (!raw) return null
  const byId = raw.rateLimitsByLimitId as Record<string, LimitBucket> | undefined
  return byId?.codex ?? raw.rateLimits as LimitBucket | undefined ?? null
}

function compatibleResources(page: PageCapability | undefined, slotId: string, resources: ResourceSummary[]) {
  const slot = page?.slots.find((item) => item.id === slotId)
  if (!slot || slot.status !== 'active') return []
  return resources.filter((resource) => (
    resource.schema_id === slot.schema_id && resource.schema_version === slot.schema_version
  ))
}

function formatPlanName(value?: string) {
  if (!value) return 'GPT —'
  const normalized = value.toLowerCase().replace(/[\s_-]/g, '')
  if (normalized.includes('plus')) return 'GPT Plus'
  if (normalized === 'pro' || normalized.includes('chatgptpro')) return 'GPT Pro'
  if (normalized.includes('free')) return 'GPT Free'
  if (normalized.includes('business')) return 'GPT Business'
  if (normalized.includes('enterprise')) return 'GPT Enterprise'
  if (normalized.includes('team')) return 'GPT Team'
  if (normalized === 'unknown') return 'GPT —'
  return value
}

function formatWindowName(window?: WindowLimit | null) {
  if (!window?.windowDurationMins) return '未知'
  if (window.windowDurationMins === 5 * 60) return '5 小时'
  if (window.windowDurationMins === 7 * 24 * 60) return '7 天'
  return '未知'
}

function QuotaWindow({ title, window }: { title: string; window?: WindowLimit | null }) {
  const used = Math.max(0, Math.min(100, window?.usedPercent ?? 0))
  const remaining = window ? 100 - used : null
  return (
    <div className="quota-window">
      <div className="quota-head"><span>{title}</span><small>{window?.windowDurationMins ? `${window.windowDurationMins} min` : '—'}</small></div>
      <div className="quota-number">{remaining === null ? '—' : remaining}<small>{remaining === null ? '' : '%'}</small></div>
      <div className="quota-track"><i style={{ width: `${remaining ?? 0}%` }} /></div>
      <div className="quota-foot"><span>剩余</span><span>重置 {formatTime(window?.resetsAt)}</span></div>
    </div>
  )
}

function NumberField({ label, value, min, max, onChange, disabled }: {
  label: string; value: number; min: number; max: number; onChange: (value: number) => void; disabled?: boolean
}) {
  return (
    <label className="field"><span>{label}</span><Input type="number" min={min} max={max} value={value} disabled={disabled}
      onChange={(event) => onChange(Number(event.target.value))} /></label>
  )
}

function FeishuProjectConfigPanel() {
  const [draft, setDraft] = useState<FeishuProjectConfig | null>(null)
  const [preview, setPreview] = useState<FeishuProjectPreview | null>(null)
  const [busy, setBusy] = useState<'load' | 'test' | 'save' | null>('load')

  useEffect(() => {
    let active = true
    agentApi.getFeishuProjectConfig()
      .then(({ config }) => { if (active) setDraft(config) })
      .catch((error) => { if (active) toast.error(errorText(error)) })
      .finally(() => { if (active) setBusy(null) })
    return () => { active = false }
  }, [])

  async function testDraft() {
    if (!draft || busy) return
    setBusy('test')
    try {
      const result = await agentApi.testFeishuProjectConfig(draft)
      setPreview(result.preview)
      toast.success('Meegle 查询与表达式执行成功')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  async function saveDraft() {
    if (!draft || busy) return
    setBusy('save')
    try {
      const result = await agentApi.saveFeishuProjectConfig(draft)
      setDraft(result.config)
      toast.success('飞书项目配置已保存')
    } catch (error) {
      toast.error(errorText(error))
    } finally {
      setBusy(null)
    }
  }

  return (
    <section className="surface feishu-config">
      <SectionTitle icon={ListChecks} title="飞书项目投影" detail="本机私有配置 · JMESPath"
        action={<div className="feishu-actions">
          <Button variant="outline" size="sm" disabled={!draft || !!busy} onClick={() => void testDraft()}>
            {busy === 'test' ? <LoaderCircle className="spin" /> : <FlaskConical />}测试
          </Button>
          <Button size="sm" disabled={!draft || !!busy} onClick={() => void saveDraft()}>
            {busy === 'save' ? <LoaderCircle className="spin" /> : <Save />}保存
          </Button>
        </div>} />
      {draft ? <>
        <div className="settings-rows">
          <div className="setting-toggle"><div><b>启用同步</b><span>{draft.enabled ? '参与定时与电池自动同步' : '保留配置但不执行查询'}</span></div><Switch checked={draft.enabled}
            onCheckedChange={(enabled) => setDraft({ ...draft, enabled })} /></div>
        </div>
        <div className="feishu-fields">
          <label className="field"><span>展示名</span><Input maxLength={32} value={draft.display_name}
            onChange={(event) => setDraft({ ...draft, display_name: event.target.value })} /></label>
          <label className="field feishu-command-field"><span>Meegle CLI 命令</span><textarea spellCheck={false} value={draft.command}
            placeholder="meegle workitem query ... --format json"
            onChange={(event) => setDraft({ ...draft, command: event.target.value })} /></label>
          <div className="field-grid two">
            <label className="field"><span>主值 JMESPath</span><Input value={draft.value_expression} placeholder="length(data)"
              onChange={(event) => setDraft({ ...draft, value_expression: event.target.value })} /></label>
            <label className="field"><span>详情 JMESPath</span><Input value={draft.detail_expression} placeholder="session_id"
              onChange={(event) => setDraft({ ...draft, detail_expression: event.target.value })} /></label>
          </div>
        </div>
        <div className="feishu-preview">
          <span>TEST OUTPUT</span>
          <div><b>{preview?.display_name ?? '尚未测试'}</b><strong>{preview?.value ?? '--'}</strong></div>
          <p>{preview?.detail ?? '—'}</p>
          <code>{preview ? `${preview.elapsed_ms}ms · ${preview.output_bytes} bytes` : 'JMESPath preview'}</code>
        </div>
      </> : <EmptyState>正在读取飞书项目配置</EmptyState>}
    </section>
  )
}

const RESOURCE_TEMPLATE = JSON.stringify({
  key: 'example/default',
  schema_id: 'example.card',
  schema_version: 1,
  revision: 1,
  updated_at: Math.floor(Date.now() / 1000),
  ttl_sec: 600,
  persistence: 'snapshot',
  payload: {},
}, null, 2)

function App() {
  const [view, setView] = useState<View>('overview')
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [configDraft, setConfigDraft] = useState<DeviceConfig | null>(null)
  const [bootError, setBootError] = useState<string | null>(null)
  const [streamDown, setStreamDown] = useState(false)
  const [operation, setOperation] = useState<string | null>(null)
  const [pageId, setPageId] = useState('')
  const [pageBindings, setPageBindings] = useState<Record<string, string>>({})
  const [resourceEditor, setResourceEditor] = useState(RESOURCE_TEMPLATE)
  const [resourceDetail, setResourceDetail] = useState<JsonObject | null>(null)
  const [detailOpen, setDetailOpen] = useState(false)
  const [enrollmentUntil, setEnrollmentUntil] = useState(0)
  const [resetOpen, setResetOpen] = useState(false)
  const [resetCode, setResetCode] = useState('')
  const [logLevel, setLogLevel] = useState<LogLevel>('all')
  const [logScope, setLogScope] = useState('all')
  const [followLogs, setFollowLogs] = useState(true)
  const logTableRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let unsubscribe = () => {}
    let cancelled = false
    async function boot() {
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
  useEffect(() => {
    if (!config) return
    setConfigDraft(structuredClone(config))
    setPageId(config.page.id)
    setPageBindings({ ...config.page.bindings })
  }, [config?.revision])

  async function perform(key: string, action: () => Promise<unknown>, success: string, refresh = false) {
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

  const codex = snapshot?.producers.find((producer) => producer.id === 'codex.usage')
  const codexPlan = typeof codex?.details.plan_type === 'string' ? codex.details.plan_type : undefined
  const codexEmail = typeof codex?.details.email === 'string' ? codex.details.email : undefined
  const codexPath = typeof codex?.details.codex_path === 'string' ? codex.details.codex_path : undefined
  const bucket = useMemo(() => getCodexBucket(snapshot), [codex?.details.rate_limits])
  const connected = snapshot?.device.phase === 'connected'
  const owner = snapshot?.device.role === 'owner'
  const pages = snapshot?.device.capabilities?.pages ?? []
  const resources = snapshot?.device.resources ?? []
  const selectedPage = pages.find((item) => item.id === pageId)
  const requiredBindingsReady = selectedPage?.slots.every((slot) => (
    slot.status !== 'active' || !slot.required || Boolean(pageBindings[slot.id])
  )) ?? false
  const page = NAV_ITEMS.find((item) => item.id === view) ?? NAV_ITEMS[0]
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

  useEffect(() => {
    if (followLogs) logTableRef.current?.scrollTo({ top: 0, behavior: 'smooth' })
  }, [snapshot?.logs[0]?.at, snapshot?.logs[0]?.message, followLogs])

  async function saveConfiguration() {
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

  async function inspectResource(resource: ResourceSummary) {
    if (!await perform(`inspect:${resource.key}`, async () => {
      const response = await agentApi.getResource(resource.key)
      setResourceDetail(response.result.resource)
      setDetailOpen(true)
    }, '资源已读取')) return
  }

  async function editResource(resource: ResourceSummary) {
    await perform(`edit:${resource.key}`, async () => {
      const response = await agentApi.getResource(resource.key)
      setResourceEditor(JSON.stringify(response.result.resource, null, 2))
    }, '资源已载入编辑器')
  }

  function choosePage(id: string) {
    const capability = pages.find((item) => item.id === id)
    const bindings: Record<string, string> = {}
    for (const slot of capability?.slots ?? []) {
      if (slot.status !== 'active') continue
      const compatible = compatibleResources(capability, slot.id, resources)
      const current = config?.page.bindings[slot.id]
      bindings[slot.id] = compatible.some((resource) => resource.key === current)
        ? current ?? ''
        : slot.required ? compatible[0]?.key ?? '' : ''
    }
    setPageId(id)
    setPageBindings(bindings)
  }

  async function publishEditedResource() {
    let resource: JsonObject
    try {
      resource = JSON.parse(resourceEditor) as JsonObject
    } catch {
      toast.error('资源 JSON 格式无效')
      return
    }
    await perform('resource.put', () => agentApi.putResource(resource), '资源已发布', true)
  }

  async function openEnrollment() {
    await perform('enrollment', async () => {
      const response = await agentApi.setEnrollment(true)
      setEnrollmentUntil(Date.now() + (response.result.expires_in_sec ?? 120) * 1000)
    }, 'Enrollment 已开放')
  }

  async function prepareReset() {
    await perform('reset.prepare', async () => {
      await agentApi.prepareFactoryReset()
      setResetCode('')
      setResetOpen(true)
    }, '确认码已显示在设备上')
  }

  function scanDevices() {
    void perform('ble.scan', agentApi.scanDevices, '设备扫描已开始')
  }

  function connectDevice(candidate: BleCandidate) {
    void perform(
      `ble.connect:${candidate.id}`,
      () => agentApi.connectDevice(candidate.id),
      `正在连接 ${candidate.name}`,
    )
  }

  function disconnectDevice() {
    void perform('ble.disconnect', agentApi.disconnectDevice, '设备连接已停止')
  }

  function autoConnectDevice() {
    void perform('ble.auto', agentApi.autoConnectDevice, '自动连接已启动')
  }

  if (bootError) {
    return (
      <main className="boot-screen">
        <div className="brand-mark"><span /><span /><span /></div>
        <p>EPD AGENT / LOCAL</p>
        <h1>无法建立本机会话</h1>
        <div className="boot-error">{bootError}</div>
      </main>
    )
  }

  if (!snapshot) {
    return <main className="boot-screen"><LoaderCircle className="boot-spinner" /><p>EPD AGENT / LOCAL</p><h1>正在连接工作台</h1></main>
  }

  return (
    <div className="workbench">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark"><span /><span /><span /></div>
          <div><strong>EPD KIT</strong><small>BLE v4 AGENT</small></div>
        </div>
        <nav aria-label="工作台导航">
          {NAV_ITEMS.map(({ id, label, icon: Icon }) => (
            <button key={id} className={view === id ? 'active' : ''} onClick={() => setView(id)} title={label}>
              <Icon /><span>{label}</span><ChevronRight />
            </button>
          ))}
        </nav>
        <div className="sidebar-state">
          <div><Bluetooth /><span>DEVICE</span><StatusPill phase={snapshot.device.phase} /></div>
          <div><Bot /><span>PRODUCERS</span><StatusPill phase={snapshot.producers.every((producer) => producer.phase === 'ready') ? 'ready' : 'starting'} /></div>
        </div>
        <div className="agent-version">AGENT {snapshot.agent.version}<br />{snapshot.agent.platform.toUpperCase()}</div>
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div><page.icon /><span>{page.label}</span></div>
          <div className="top-actions">
            {streamDown ? <span className="stream-warning"><Radio />事件流重连中</span> : null}
            <Button variant="ghost" size="icon" title="重新读取设备" disabled={!connected || !!operation}
              onClick={() => void perform('reload', agentApi.reloadDevice, '设备状态已更新', true)}>
              <RefreshCw className={operation === 'reload' ? 'spin' : ''} />
            </Button>
            <Button variant={snapshot.agent.paused ? 'signal' : 'outline'} size="sm" disabled={!!operation}
              onClick={() => void perform('pause', () => agentApi.setPaused(!snapshot.agent.paused), snapshot.agent.paused ? '同步已恢复' : '同步已暂停', true)}>
              {snapshot.agent.paused ? <Play /> : <Pause />}{snapshot.agent.paused ? '恢复' : '暂停'}
            </Button>
          </div>
        </header>

        <div className="page-body">
          <div className="page-heading"><div><span>LOCAL DEVICE WORKBENCH</span><h1>{page.label}</h1></div></div>

          {view === 'overview' ? <>
            <DeviceConnectionPanel
              device={snapshot.device}
              operation={operation}
              onScan={scanDevices}
              onConnect={connectDevice}
              onDisconnect={disconnectDevice}
              onAutoConnect={autoConnectDevice}
            />
            <div className="metrics-grid">
              <Metric label="BLE 设备" value={PHASE_LABELS[snapshot.device.phase] ?? snapshot.device.phase}
                detail={snapshot.device.name ?? '等待发现 EPD-KIT'} icon={Bluetooth} tone={connected ? 'green' : 'red'} />
              <Metric label="Producer" value={`${snapshot.producers.filter((producer) => producer.phase === 'ready').length} / ${snapshot.producers.length}`}
                detail={codex?.last_error ?? '编译期 Producer Registry'} icon={Bot} tone={snapshot.producers.every((producer) => producer.phase === 'ready') ? 'cyan' : 'default'} />
              <Metric label="同步" value={formatAge(codex?.last_sync_at)}
                detail={`下一次 ${formatTime(codex?.next_sync_at)}`} icon={Clock3} />
              <Metric label="资源" value={String(resources.length)}
                detail={`${config?.page.id ?? '无 page'} / rev ${config?.revision ?? '—'}`} icon={Database} />
            </div>
            <div className="overview-grid">
              <section className="surface epd-module">
                <SectionTitle icon={MonitorCog} title="当前墨水屏页面" detail={`${config?.page.id ?? '未选择'} · ${Object.values(config?.page.bindings ?? {}).join(', ')}`}
                  action={<Button size="sm" disabled={!connected || !!operation} onClick={() => void perform('display', () => agentApi.refreshDisplay('auto'), '刷新已排队')}><RefreshCw />刷新</Button>} />
                <div className="epd-frame">
                  <div className="epd-screen">
                    <div className="epd-top">
                      <div className="epd-brand"><b>{config?.page.id === 'home' ? new Date().toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : 'Codex'}</b><i>-</i><span>{formatPlanName(bucket?.planType ?? codexPlan)}</span></div>
                      <div className="epd-indicators">{configDraft?.hardware.battery.enabled ? <small>--%</small> : null}<i className={connected ? 'connected' : ''} /></div>
                    </div>
                    <div className="epd-rule" />
                    <div className="epd-quotas"><QuotaWindow title={formatWindowName(bucket?.primary)} window={bucket?.primary} /><QuotaWindow title={formatWindowName(bucket?.secondary)} window={bucket?.secondary} /></div>
                    <div className="epd-bottom"><b>{codex?.phase === 'ready' ? '同步正常' : '数据保留'}</b><span>{config?.page.id === 'home' ? '飞书项目 未配置' : `更新 ${formatTime(codex?.last_sync_at)}`}</span></div>
                  </div>
                </div>
                <div className="command-row">
                  <Button variant="outline" size="sm" disabled={!connected || !!operation} onClick={() => void perform('full', () => agentApi.refreshDisplay('full'), '全刷已排队')}><RotateCcw />全刷</Button>
                  <Button variant="outline" size="sm" disabled={!!operation || !codex} onClick={() => void perform('producer:codex.usage', () => agentApi.refreshProducer('codex.usage'), 'Codex 读取已排队')}><Bot />同步额度</Button>
                  <Button variant="outline" size="sm" disabled={!owner || !!operation} onClick={() => void perform('restart', agentApi.restartDevice, '设备即将重启')}><Power />重启设备</Button>
                </div>
              </section>
              <section className="surface event-module">
                <SectionTitle icon={Activity} title="最近活动" detail="Agent 与设备事件" />
                <div className="activity-list">
                  {snapshot.logs.slice(0, 8).map((entry, index) => <div key={`${entry.at}:${index}`}>
                    <i className={entry.level} /><time>{formatTime(entry.at)}</time><b>{entry.scope}</b><span>{entry.message}</span>
                  </div>)}
                  {!snapshot.logs.length ? <EmptyState>暂无活动记录</EmptyState> : null}
                </div>
              </section>
            </div>
          </> : null}

          {view === 'hardware' && configDraft ? <>
            <div className="page-actions"><Button className="save-button" disabled={!owner || !!operation} onClick={() => void saveConfiguration()}><Save />保存配置</Button></div>
            <section className="surface settings-section">
              <SectionTitle icon={BatteryCharging} title="电池与 IO" detail="硬件字段提交后重启生效" />
              <div className="settings-rows">
                <div className="setting-toggle"><div><b>电池输入</b><span>ADC、电池服务与低电量保护</span></div><Switch checked={configDraft.hardware.battery.enabled}
                  onCheckedChange={(enabled) => setConfigDraft({ ...configDraft, hardware: { ...configDraft.hardware, battery: { ...configDraft.hardware.battery, enabled } } })} /></div>
                <div className="field-grid three">
                  <NumberField label="低电量 / mV" min={3001} max={4298} value={configDraft.hardware.battery.low_mv} disabled={!configDraft.hardware.battery.enabled}
                    onChange={(low_mv) => setConfigDraft({ ...configDraft, hardware: { ...configDraft.hardware, battery: { ...configDraft.hardware.battery, low_mv } } })} />
                  <NumberField label="临界电量 / mV" min={3000} max={4297} value={configDraft.hardware.battery.critical_mv} disabled={!configDraft.hardware.battery.enabled}
                    onChange={(critical_mv) => setConfigDraft({ ...configDraft, hardware: { ...configDraft.hardware, battery: { ...configDraft.hardware.battery, critical_mv } } })} />
                  <NumberField label="恢复电量 / mV" min={3002} max={4300} value={configDraft.hardware.battery.recovery_mv} disabled={!configDraft.hardware.battery.enabled}
                    onChange={(recovery_mv) => setConfigDraft({ ...configDraft, hardware: { ...configDraft.hardware, battery: { ...configDraft.hardware.battery, recovery_mv } } })} />
                </div>
                <div className="setting-toggle"><div><b>IO12 按键</b><span>{configDraft.hardware.io12.mode === 'key' ? '短按触发立即同步' : '引脚保持高阻输入'}</span></div><Switch checked={configDraft.hardware.io12.mode === 'key'}
                  onCheckedChange={(enabled) => setConfigDraft({ ...configDraft, hardware: { ...configDraft.hardware, io12: { mode: enabled ? 'key' : 'disabled' } } })} /></div>
              </div>
            </section>
            <section className="surface settings-section">
              <SectionTitle icon={Zap} title="功耗与显示" detail="广播策略及电子纸全刷阈值" />
              <div className="settings-rows">
                <label className="field"><span>功耗档位</span><select value={configDraft.power.profile} onChange={(event) => setConfigDraft({ ...configDraft, power: { ...configDraft.power, profile: event.target.value as 'mains' | 'battery' } })}><option value="mains">mains / 常在线</option><option value="battery">battery / 同步后休眠</option></select></label>
                <NumberField label="休眠唤醒周期 / 秒" min={60} max={86400} value={configDraft.power.wake_interval_sec} disabled={configDraft.power.profile !== 'battery'}
                  onChange={(wake_interval_sec) => setConfigDraft({ ...configDraft, power: { ...configDraft.power, wake_interval_sec } })} />
                <div className="field-grid three">
                  <NumberField label="局刷后全刷 / 次" min={1} max={100} value={configDraft.display.full_after_partial_count}
                    onChange={(full_after_partial_count) => setConfigDraft({ ...configDraft, display: { ...configDraft.display, full_after_partial_count } })} />
                  <NumberField label="全刷最大间隔 / 秒" min={3600} max={604800} value={configDraft.display.full_max_age_sec}
                    onChange={(full_max_age_sec) => setConfigDraft({ ...configDraft, display: { ...configDraft.display, full_max_age_sec } })} />
                  <NumberField label="全刷面积阈值 / %" min={10} max={100} value={configDraft.display.full_area_threshold_percent}
                    onChange={(full_area_threshold_percent) => setConfigDraft({ ...configDraft, display: { ...configDraft.display, full_area_threshold_percent } })} />
                </div>
              </div>
            </section>
            <section className="surface settings-section">
              <SectionTitle icon={Settings2} title="区域与标识" />
              <div className="field-grid three settings-fields">
                <label className="field"><span>设备名称</span><Input value={configDraft.device.name} onChange={(event) => setConfigDraft({ ...configDraft, device: { ...configDraft.device, name: event.target.value } })} /></label>
                <label className="field"><span>Locale</span><Input value={configDraft.device.locale} onChange={(event) => setConfigDraft({ ...configDraft, device: { ...configDraft.device, locale: event.target.value } })} /></label>
                <label className="field"><span>IANA 时区</span><Input value={configDraft.device.timezone_iana} onChange={(event) => setConfigDraft({ ...configDraft, device: { ...configDraft.device, timezone_iana: event.target.value } })} /></label>
              </div>
            </section>
          </> : null}

          {view === 'resources' ? <>
            <section className="surface settings-section">
              <SectionTitle icon={MonitorCog} title="Page 与 Binding" detail={`配置 revision ${config?.revision ?? '—'}`}
                action={<Button size="sm" disabled={!owner || !pageId || !requiredBindingsReady || !!operation}
                  onClick={() => void perform('page', () => agentApi.setPage({ id: pageId, bindings: Object.fromEntries(Object.entries(pageBindings).filter(([, key]) => key)) }), '页面已切换', true)}><Save />应用</Button>} />
              <div className="settings-fields page-binding-editor">
                <label className="field"><span>Page</span><select value={pageId} onChange={(event) => choosePage(event.target.value)}>{pages.map((item) => <option key={item.id} value={item.id}>{item.title} · {item.id}</option>)}</select></label>
                <div className="slot-grid">
                  {selectedPage?.slots.map((slot) => {
                    const compatible = compatibleResources(selectedPage, slot.id, resources)
                    if (slot.status === 'reserved') return <label className="field" key={slot.id}><span>{slot.id} · reserved</span><select disabled><option>未配置（不可绑定）</option></select></label>
                    return <label className="field" key={slot.id}><span>{slot.id} · {slot.schema_id}/v{slot.schema_version}{slot.required ? ' · required' : ''}</span><select value={pageBindings[slot.id] ?? ''} onChange={(event) => setPageBindings({ ...pageBindings, [slot.id]: event.target.value })}><option value="">{slot.required ? '请选择兼容资源' : '不绑定'}</option>{compatible.map((resource) => <option key={resource.key} value={resource.key}>{resource.key}</option>)}</select></label>
                  })}
                </div>
              </div>
            </section>
            <section className="surface table-section">
              <SectionTitle icon={HardDrive} title="资源存储" detail={`${resources.length} / ${snapshot.device.capabilities?.max_resources ?? 8}`}
                action={<Button variant="outline" size="sm" disabled={!owner || !!operation} onClick={() => setResourceEditor(RESOURCE_TEMPLATE)}><Braces />新建 JSON</Button>} />
              <div className="data-table resource-table">
                <div className="table-head"><span>KEY / SCHEMA</span><span>REVISION</span><span>FRESHNESS</span><span>ACTIONS</span></div>
                {resources.map((resource) => <div className="table-row" key={resource.key}>
                  <span><b>{resource.key}</b><small>{resource.schema_id}/v{resource.schema_version} · {resource.persistence}</small></span>
                  <span className="mono">{resource.revision}</span><span>{formatAge(resource.updated_at)}<small>TTL {resource.ttl_sec}s</small></span>
                  <span className="row-actions"><Button variant="ghost" size="icon" title="查看资源" disabled={!!operation} onClick={() => void inspectResource(resource)}><Eye /></Button><Button variant="ghost" size="icon" title="载入编辑器" disabled={!owner || !!operation} onClick={() => void editResource(resource)}><Braces /></Button><Button variant="ghost" size="icon" title="删除资源" disabled={!owner || !!operation || Object.values(config?.page.bindings ?? {}).includes(resource.key)} onClick={() => void perform(`delete:${resource.key}`, () => agentApi.deleteResource(resource.key), '资源已删除', true)}><Trash2 /></Button></span>
                </div>)}
                {!resources.length ? <EmptyState>设备中没有资源</EmptyState> : null}
              </div>
            </section>
            <section className="surface resource-editor-section">
              <SectionTitle icon={Braces} title="Resource JSON" detail="Owner 调试入口"
                action={<Button size="sm" disabled={!owner || !!operation} onClick={() => void publishEditedResource()}><Save />PUT</Button>} />
              <textarea className="resource-editor" spellCheck={false} value={resourceEditor} onChange={(event) => setResourceEditor(event.target.value)} />
            </section>
          </> : null}

          {view === 'producers' ? <>
            {snapshot.producers.map((producer) => <Fragment key={producer.id}>
              <section className="surface producer-status">
                <SectionTitle icon={Bot} title={producer.title} detail={`${producer.id} · ${producer.resource_keys.join(', ')}`}
                  action={<Button size="sm" disabled={!!operation} onClick={() => void perform(`producer:${producer.id}`, () => agentApi.refreshProducer(producer.id), `${producer.title} 已排队`)}><RefreshCw />立即刷新</Button>} />
                <div className="account-line"><StatusPill phase={producer.phase} /><div><b>{producer.id === 'codex.usage' ? codexEmail ?? '未识别账号' : producer.title}</b><span>{producer.id === 'codex.usage' ? formatPlanName(codexPlan) : producer.resource_keys.join(', ')}</span></div><div><small>最近同步</small><b>{formatTime(producer.last_sync_at)}</b></div></div>
                {producer.last_error ? <div className="inline-error"><CircleAlert />{producer.last_error}</div> : null}
                <pre className="producer-details">{JSON.stringify(producer.details, null, 2)}</pre>
              </section>
              {producer.id === 'feishu.project' ? <FeishuProjectConfigPanel /> : null}
            </Fragment>)}
            {!snapshot.producers.length ? <section className="surface"><EmptyState>没有已注册 Producer</EmptyState></section> : null}
            {codex ? <section className="surface quota-section">
              <SectionTitle icon={Gauge} title={bucket?.limitName ?? 'Codex 额度'} detail={bucket?.rateLimitReachedType ? `LIMIT: ${bucket.rateLimitReachedType}` : '当前计量窗口'} />
              <div className="quota-grid"><QuotaWindow title="主窗口" window={bucket?.primary} /><QuotaWindow title="次窗口" window={bucket?.secondary} /></div>
              {!bucket ? <EmptyState>暂无额度快照</EmptyState> : null}
            </section> : null}
            {codex ? <section className="surface facts-section"><div><span>Codex 路径</span><code>{codexPath ?? '未找到'}</code></div><div><span>资源 schema</span><code>codex.rate_limits/v1</code></div><div><span>轮询间隔</span><code>60s / backoff max 900s</code></div></section> : null}
          </> : null}

          {view === 'security' ? <>
            <section className="surface trust-intro">
              <SectionTitle icon={KeyRound} title="Owner 与 Enrollment" detail={`当前连接角色：${snapshot.device.role ?? '—'}`}
                action={<Button size="sm" disabled={!owner || !!operation} onClick={() => void openEnrollment()}><Users />开放 120 秒</Button>} />
              {enrollmentUntil > Date.now() ? <div className="enrollment-active"><Radio />ENROLLMENT ACTIVE <span>{formatTime(Math.floor(enrollmentUntil / 1000))}</span><Button variant="ghost" size="sm" onClick={() => void perform('enrollment.close', () => agentApi.setEnrollment(false), 'Enrollment 已关闭')}>关闭</Button></div> : null}
            </section>
            <section className="surface table-section">
              <SectionTitle icon={Users} title="Bond 列表" detail={`${snapshot.device.bonds.length} / 4`} />
              <div className="data-table bonds-table">
                <div className="table-head"><span>HOST ID</span><span>ROLE</span><span>ACTIONS</span></div>
                {snapshot.device.bonds.map((bond) => <div className="table-row" key={bond.id}><span><b>{bond.id}</b></span><span><span className={`role ${bond.role}`}>{bond.role}</span></span><span className="row-actions">{bond.role !== 'owner' ? <Button variant="outline" size="sm" disabled={!owner || !!operation} onClick={() => void perform(`owner:${bond.id}`, () => agentApi.transferOwner(bond.id), 'Owner 已转移', true)}>设为 Owner</Button> : null}<Button variant="ghost" size="icon" title="撤销 bond" disabled={!owner || bond.role === 'owner' || !!operation} onClick={() => void perform(`revoke:${bond.id}`, () => agentApi.revokeBond(bond.id), 'Bond 已撤销', true)}><Trash2 /></Button></span></div>)}
                {!snapshot.device.bonds.length ? <EmptyState>暂无已保存的 bond</EmptyState> : null}
              </div>
            </section>
          </> : null}

          {view === 'diagnostics' ? <>
            <section className="surface agent-controls">
              <SectionTitle icon={Cpu} title="Agent" detail={`v${snapshot.agent.version} · ${snapshot.agent.platform}`} />
              <div className="settings-rows">
                <div className="setting-toggle"><div><b>登录时启动</b><span>{snapshot.agent.autostart_enabled ? '当前用户启动项已启用' : '当前用户启动项已关闭'}</span></div><Switch checked={snapshot.agent.autostart_enabled} disabled={!!operation} onCheckedChange={(enabled) => void perform('autostart', () => agentApi.setAutostart(enabled), enabled ? '自启动已启用' : '自启动已关闭', true)} /></div>
                <div className="setting-toggle"><div><b>BLE 同步</b><span>{snapshot.agent.paused ? '扫描与同步已暂停' : '自动发现与同步运行中'}</span></div><Switch checked={!snapshot.agent.paused} disabled={!!operation} onCheckedChange={(enabled) => void perform('pause', () => agentApi.setPaused(!enabled), enabled ? '同步已恢复' : '同步已暂停', true)} /></div>
              </div>
            </section>
            <section className="surface diagnostics-section">
              <SectionTitle icon={Braces} title="设备诊断" />
              <pre>{JSON.stringify(snapshot.device.diagnostics ?? { phase: snapshot.device.phase, error: snapshot.device.last_error ?? null }, null, 2)}</pre>
            </section>
            <section className="surface log-section">
              <SectionTitle icon={Activity} title="核心服务日志" detail={`${visibleLogs.length} / ${snapshot.logs.length}`}
                action={<span className={`console-live ${streamDown ? 'down' : ''}`}><i />{streamDown ? 'RECONNECTING' : 'LIVE'}</span>} />
              <div className="console-toolbar">
                <div className="log-levels" aria-label="日志级别">
                  {(['all', 'info', 'warn', 'error'] as const).map((level) => <button type="button" key={level} className={logLevel === level ? 'active' : ''} onClick={() => setLogLevel(level)}>{level}</button>)}
                </div>
                <label className="scope-filter"><span>SCOPE</span><select value={logScope} onChange={(event) => setLogScope(event.target.value)}><option value="all">all</option>{logScopes.map((scope) => <option key={scope} value={scope}>{scope}</option>)}</select></label>
                <label className="console-follow"><span>自动跟随</span><Switch checked={followLogs} onCheckedChange={setFollowLogs} /></label>
              </div>
              <div className="log-table" ref={logTableRef}>{visibleLogs.map((entry, index) => <div key={`${entry.at}:${entry.scope}:${entry.message}:${index}`}><time>{formatTime(entry.at)}</time><span className={entry.level}>{entry.level}</span><b>{entry.scope}</b><code>{entry.message}</code></div>)}{!visibleLogs.length ? <EmptyState>当前筛选没有日志</EmptyState> : null}</div>
            </section>
            <section className="danger-zone">
              <div><Trash2 /><span><b>恢复出厂</b><small>清除 v4 配置、资源、owner 与全部 bond</small></span></div>
              <Button variant="destructive" size="sm" disabled={!owner || !!operation} onClick={() => void prepareReset()}>准备恢复</Button>
            </section>
          </> : null}
        </div>
      </main>

      <Dialog open={detailOpen} onOpenChange={setDetailOpen}><DialogContent><DialogHeader><DialogTitle>资源内容</DialogTitle><DialogDescription>设备返回的完整语义资源</DialogDescription></DialogHeader><pre className="dialog-json">{JSON.stringify(resourceDetail, null, 2)}</pre><DialogFooter><DialogClose asChild><Button variant="outline">关闭</Button></DialogClose></DialogFooter></DialogContent></Dialog>
      <Dialog open={resetOpen} onOpenChange={setResetOpen}><DialogContent><DialogHeader><DialogTitle>确认恢复出厂</DialogTitle><DialogDescription>输入墨水屏显示的六位确认码。确认后设备会清除 v4 namespace 与全部 bond 并重启。</DialogDescription></DialogHeader><label className="field"><span>六位确认码</span><Input inputMode="numeric" maxLength={6} value={resetCode} onChange={(event) => setResetCode(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><DialogFooter><DialogClose asChild><Button variant="outline">取消</Button></DialogClose><Button variant="destructive" disabled={resetCode.length !== 6 || !!operation} onClick={() => void perform('reset.commit', () => agentApi.commitFactoryReset(Number(resetCode)), '设备已恢复出厂').then((ok) => ok && setResetOpen(false))}>确认清除</Button></DialogFooter></DialogContent></Dialog>
      <Toaster position="bottom-right" richColors closeButton />
    </div>
  )
}

export default App
