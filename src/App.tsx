import { useEffect, useRef, useState, type ReactNode } from 'react'
import {
  Activity,
  AppWindow,
  BatteryCharging,
  Bluetooth,
  Check,
  ChevronRight,
  Circle,
  Clock3,
  Cpu,
  Eye,
  EyeOff,
  Gauge,
  KeyRound,
  LayoutDashboard,
  LoaderCircle,
  LockKeyhole,
  LogOut,
  Network,
  Radio,
  RefreshCw,
  RotateCcw,
  Save,
  ScanLine,
  Settings2,
  ShieldCheck,
  SignalHigh,
  SlidersHorizontal,
  Server,
  Trash2,
  Wifi,
  WifiOff,
  Zap,
  type LucideIcon,
} from 'lucide-react'
import { Toaster, toast } from 'sonner'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from './components/ui/dialog'
import { Input } from './components/ui/input'
import { Label } from './components/ui/label'
import { Progress } from './components/ui/progress'
import { Switch } from './components/ui/switch'
import {
  ToolkitBleClient,
  ToolkitError,
  type BleActivity,
  type DeviceStatus,
  type HelloResult,
  type StandardDeviceInfo,
  type ToolkitApp,
  type ToolkitConfig,
  type WifiNetwork,
  type WifiTestResult,
} from './lib/ble'
import { cn } from './lib/utils'

type View = 'overview' | 'network' | 'codex' | 'system'
type ConnectionPhase = 'idle' | 'connecting' | 'connected'

const DEFAULT_CONFIG: ToolkitConfig = {
  version: 1,
  device: {
    name: 'epd-kit',
    locale: 'zh-CN',
    timezone: { iana: 'Asia/Shanghai', posix: 'CST-8' },
  },
  wifi: {
    ssid: '',
    password_set: false,
    ipv4: { mode: 'dhcp', address: '', gateway: '', subnet: '', dns1: '', dns2: '' },
  },
  power: {
    poll_interval_sec: 300,
    ble_window_sec: 180,
    offline_backoff_sec: [300, 900, 1800, 3600],
  },
  display: {
    full_after_partial_count: 12,
    full_max_age_sec: 86400,
    full_area_threshold_percent: 40,
  },
  battery: { low_mv: 3550, critical_mv: 3400, recovery_mv: 3650 },
  active_app: 'codex_usage',
  apps: {
    codex_usage: {
      account_id: '',
      expires_at: 0,
      access_token_set: false,
      proxy: {
        enabled: false,
        host: '',
        port: 8080,
        username: '',
        password_set: false,
      },
    },
  },
}

const NAV_ITEMS: Array<{ id: View; label: string; shortLabel: string; icon: LucideIcon }> = [
  { id: 'overview', label: '设备概览', shortLabel: '概览', icon: LayoutDashboard },
  { id: 'network', label: '网络配置', shortLabel: '网络', icon: Wifi },
  { id: 'codex', label: 'Codex 应用', shortLabel: 'Codex', icon: AppWindow },
  { id: 'system', label: '系统参数', shortLabel: '系统', icon: SlidersHorizontal },
]

const ERROR_LABELS: Record<string, string> = {
  unsupported_version: '协议版本不兼容',
  invalid_request: '请求无效',
  unauthorized: '需要重新配对或物理确认',
  invalid_config: '配置不合法',
  busy: '设备正忙',
  timeout: '分片接收超时',
  client_timeout: '设备响应超时',
  too_large: '消息过大',
  wifi_failed: 'Wi-Fi 连接失败',
  auth_expired: 'Codex 凭据已过期',
  internal_error: '设备内部错误',
  disconnected: '设备已断开',
  unsupported_browser: '浏览器不支持 Web Bluetooth',
}

function errorMessage(error: unknown) {
  if (error instanceof ToolkitError) {
    const title = ERROR_LABELS[error.code] ?? error.code
    return error.message && error.message !== title ? `${title} · ${error.message}` : title
  }
  return error instanceof Error ? error.message : String(error)
}

function formatUptime(milliseconds?: number) {
  if (milliseconds === undefined) return '—'
  const totalMinutes = Math.floor(milliseconds / 60_000)
  if (totalMinutes < 60) return `${totalMinutes}m`
  const hours = Math.floor(totalMinutes / 60)
  return `${hours}h ${totalMinutes % 60}m`
}

function formatExpiry(seconds: number) {
  if (!seconds) return '未知'
  return new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }).format(new Date(seconds * 1000))
}

function toDateTimeInput(seconds: number) {
  if (!seconds) return ''
  const date = new Date(seconds * 1000)
  const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000)
  return shifted.toISOString().slice(0, 16)
}

function fromDateTimeInput(value: string) {
  return value ? Math.floor(new Date(value).getTime() / 1000) : 0
}

function rssiBars(rssi: number) {
  if (rssi >= -55) return 4
  if (rssi >= -67) return 3
  if (rssi >= -75) return 2
  return 1
}

function Panel({ children, className }: { children: ReactNode; className?: string }) {
  return <section className={cn('panel', className)}>{children}</section>
}

function PanelTitle({ icon: Icon, title, action }: { icon: LucideIcon; title: string; action?: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3.5 sm:px-5">
      <div className="flex items-center gap-2.5">
        <span className="grid size-7 place-items-center rounded-sm border border-border bg-muted">
          <Icon className="size-3.5" />
        </span>
        <h2 className="font-display text-sm font-black tracking-[.04em]">{title}</h2>
      </div>
      {action}
    </div>
  )
}

function Field({ label, htmlFor, children, hint }: { label: string; htmlFor: string; children: ReactNode; hint?: string }) {
  return (
    <div>
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
      {hint ? <p className="mt-1.5 text-[11px] text-muted-foreground">{hint}</p> : null}
    </div>
  )
}

function PageIntro({ eyebrow, title, action }: { eyebrow: string; title: string; action?: ReactNode }) {
  return (
    <div className="mb-5 flex items-end justify-between gap-4 sm:mb-6">
      <div>
        <p className="mb-1 font-mono text-[10px] font-bold tracking-[.2em] text-muted-foreground uppercase">{eyebrow}</p>
        <h1 className="font-display text-3xl font-black tracking-[-.035em] sm:text-4xl">{title}</h1>
      </div>
      {action}
    </div>
  )
}

function LoadingIcon({ active }: { active: boolean }) {
  return active ? <LoaderCircle className="animate-spin" /> : null
}

function App() {
  const clientRef = useRef<ToolkitBleClient | null>(null)
  if (!clientRef.current) clientRef.current = new ToolkitBleClient()
  const client = clientRef.current

  const [view, setView] = useState<View>('overview')
  const [phase, setPhase] = useState<ConnectionPhase>('idle')
  const [operation, setOperation] = useState<string | null>(null)
  const [standardInfo, setStandardInfo] = useState<StandardDeviceInfo | null>(null)
  const [hello, setHello] = useState<HelloResult | null>(null)
  const [status, setStatus] = useState<DeviceStatus | null>(null)
  const [config, setConfig] = useState<ToolkitConfig>(DEFAULT_CONFIG)
  const [apps, setApps] = useState<ToolkitApp[]>([])
  const [networks, setNetworks] = useState<WifiNetwork[]>([])
  const [wifiTestResult, setWifiTestResult] = useState<WifiTestResult | null>(null)
  const [activities, setActivities] = useState<BleActivity[]>([])
  const [staged, setStaged] = useState(false)
  const [wifiPassword, setWifiPassword] = useState('')
  const [clearWifiPassword, setClearWifiPassword] = useState(false)
  const [showWifiPassword, setShowWifiPassword] = useState(false)
  const [accessToken, setAccessToken] = useState('')
  const [clearAccessToken, setClearAccessToken] = useState(false)
  const [showAccessToken, setShowAccessToken] = useState(false)
  const [proxyPassword, setProxyPassword] = useState('')
  const [clearProxyPassword, setClearProxyPassword] = useState(false)
  const [showProxyPassword, setShowProxyPassword] = useState(false)
  const [resetOpen, setResetOpen] = useState(false)
  const [resetNonce, setResetNonce] = useState<number | null>(null)
  const [resetDeadline, setResetDeadline] = useState(0)
  const [resetTick, setResetTick] = useState(Date.now())

  const connected = phase === 'connected'
  const busy = operation !== null
  const supportsBluetooth = 'bluetooth' in navigator
  const pageName = NAV_ITEMS.find((item) => item.id === view)?.label ?? '设备概览'
  const resetSeconds = Math.max(0, Math.ceil((resetDeadline - resetTick) / 1000))

  useEffect(() => {
    client.onActivity = (entry) => setActivities((current) => [entry, ...current].slice(0, 12))
    client.onBattery = (battery) => setStandardInfo((current) => current ? { ...current, battery } : current)
    client.onDisconnected = () => {
      setPhase('idle')
      setOperation(null)
      setStaged(false)
      toast.info('设备已断开')
    }
  }, [client])

  useEffect(() => {
    if (!resetNonce) return
    const timer = window.setInterval(() => setResetTick(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [resetNonce])

  async function perform<T>(key: string, work: () => Promise<T>, success?: string): Promise<T | null> {
    if (busy) return null
    setOperation(key)
    try {
      const result = await work()
      if (success) toast.success(success)
      return result
    } catch (error) {
      toast.error(errorMessage(error))
      return null
    } finally {
      setOperation(null)
    }
  }

  async function connect() {
    if (!supportsBluetooth) {
      toast.error('请使用 Chrome 或 Edge')
      return
    }
    setPhase('connecting')
    setOperation('connect')
    try {
      const info = await client.connect()
      setStandardInfo(info)
      const helloResult = await client.transact<HelloResult>('hello')
      if (helloResult.protocol !== 1) throw new ToolkitError('unsupported_version', `设备协议为 v${helloResult.protocol}`)
      setHello(helloResult)
      const statusResult = await client.transact<DeviceStatus>('device.status')
      setStatus(statusResult)
      const configResult = await client.transact<{ config: ToolkitConfig }>('config.get')
      setConfig(configResult.config)
      const appResult = await client.transact<{ apps: ToolkitApp[] }>('app.list')
      setApps(appResult.apps)
      setPhase('connected')
      toast.success('设备已连接')
    } catch (error) {
      client.disconnect()
      setPhase('idle')
      toast.error(errorMessage(error))
    } finally {
      setOperation(null)
    }
  }

  function disconnect() {
    client.disconnect()
    setPhase('idle')
    setOperation(null)
    setStaged(false)
    toast.info('已断开')
  }

  async function reloadDevice() {
    const result = await perform('reload', async () => {
      const nextStatus = await client.transact<DeviceStatus>('device.status')
      const nextConfig = await client.transact<{ config: ToolkitConfig }>('config.get')
      const nextApps = await client.transact<{ apps: ToolkitApp[] }>('app.list')
      return { nextStatus, nextConfig, nextApps }
    }, '已同步')
    if (!result) return
    setStatus(result.nextStatus)
    setConfig(result.nextConfig.config)
    setApps(result.nextApps.apps)
    setStaged(false)
    setWifiPassword('')
    setAccessToken('')
    setProxyPassword('')
    setWifiTestResult(null)
  }

  async function patchNetwork(testAfterPatch = false) {
    const ipv4 = config.wifi.ipv4.mode === 'dhcp'
      ? { mode: 'dhcp' }
      : config.wifi.ipv4
    const wifi: Record<string, unknown> = { ssid: config.wifi.ssid, ipv4 }
    if (wifiPassword) wifi.password = wifiPassword
    if (clearWifiPassword) wifi.password = ''
    const result = await perform(testAfterPatch ? 'wifi.test' : 'wifi.patch', async () => {
      const patched = await client.transact<{ staged: boolean; configured: boolean }>('config.patch', { patch: { wifi } })
      const test = testAfterPatch ? await client.transact<WifiTestResult>('wifi.test') : null
      return { patched, test }
    }, testAfterPatch ? 'Wi-Fi 可用' : '网络配置已暂存')
    if (!result) return
    setStaged(true)
    setWifiTestResult(result.test)
    setWifiPassword('')
    setClearWifiPassword(false)
  }

  async function scanWifi() {
    const result = await perform('wifi.scan', () => client.transact<{ networks: WifiNetwork[] }>('wifi.scan'), '扫描完成')
    if (result) setNetworks(result.networks)
  }

  async function patchCodex() {
    const proxy = config.apps.codex_usage.proxy.enabled
      ? {
          enabled: true,
          host: config.apps.codex_usage.proxy.host,
          port: config.apps.codex_usage.proxy.port,
          username: config.apps.codex_usage.proxy.username,
          ...(
            config.apps.codex_usage.proxy.username
              ? proxyPassword
                ? { password: proxyPassword }
                : clearProxyPassword
                  ? { password: '' }
                  : {}
              : { password: '' }
          ),
        }
      : { enabled: false }
    const codex: Record<string, unknown> = {
      account_id: config.apps.codex_usage.account_id,
      expires_at: config.apps.codex_usage.expires_at,
      proxy,
    }
    if (accessToken) codex.access_token = accessToken
    if (clearAccessToken) codex.access_token = ''
    const result = await perform('codex.patch', () => client.transact<{ staged: boolean; configured: boolean }>('config.patch', {
      patch: { apps: { codex_usage: codex } },
    }), 'Codex 配置已暂存')
    if (!result) return
    setStaged(true)
    setAccessToken('')
    setClearAccessToken(false)
    setProxyPassword('')
    setClearProxyPassword(false)
  }

  async function activateCodex() {
    const result = await perform('app.activate', () => client.transact<{ staged: boolean }>('app.activate', { id: 'codex_usage' }), '应用已暂存')
    if (!result) return
    setStaged(true)
    setConfig((current) => ({ ...current, active_app: 'codex_usage' }))
    setApps((current) => current.map((app) => ({ ...app, active: app.id === 'codex_usage' })))
  }

  async function patchSystem() {
    const patch = {
      version: 1,
      device: config.device,
      power: config.power,
      display: config.display,
      battery: config.battery,
    }
    const result = await perform('system.patch', () => client.transact<{ staged: boolean; configured: boolean }>('config.patch', { patch }), '系统参数已暂存')
    if (result) setStaged(true)
  }

  async function commitConfig() {
    const result = await perform('config.commit', () => client.transact<Record<string, unknown>>('config.commit'), '配置已保存')
    if (!result) return
    setStaged(false)
    const nextStatus = await perform('status.after.commit', () => client.transact<DeviceStatus>('device.status'))
    if (nextStatus) setStatus(nextStatus)
  }

  async function refreshNow() {
    const result = await perform('refresh.now', () => client.transact<{ scheduled: boolean }>('refresh.now'), '刷新已排队')
    if (result?.scheduled) setActivities((current) => [{ at: Date.now(), kind: 'system' as const, label: '设备即将刷新并休眠' }, ...current].slice(0, 12))
  }

  async function prepareReset() {
    const result = await perform('factory.prepare', () => client.transact<{ nonce: number; expires_in_sec: number; physical_confirmation_required: boolean }>('factory_reset.prepare'))
    if (!result) return
    setResetNonce(result.nonce)
    setResetDeadline(Date.now() + result.expires_in_sec * 1000)
    setResetTick(Date.now())
  }

  async function commitReset() {
    if (!resetNonce || resetSeconds <= 0) return
    const result = await perform('factory.commit', () => client.transact<Record<string, unknown>>('factory_reset.commit', { nonce: resetNonce }), '设备已恢复出厂')
    if (!result) return
    setResetOpen(false)
    setResetNonce(null)
    setStaged(false)
  }

  const setPower = (key: keyof ToolkitConfig['power'], value: number | [number, number, number, number]) => {
    setConfig((current) => ({ ...current, power: { ...current.power, [key]: value } }))
  }
  const setIpv4 = (key: keyof ToolkitConfig['wifi']['ipv4'], value: string) => {
    setConfig((current) => ({ ...current, wifi: { ...current.wifi, ipv4: { ...current.wifi.ipv4, [key]: value } } }))
    setWifiTestResult(null)
  }
  const setProxy = (key: keyof ToolkitConfig['apps']['codex_usage']['proxy'], value: string | number | boolean) => {
    setConfig((current) => ({
      ...current,
      apps: { codex_usage: { ...current.apps.codex_usage, proxy: { ...current.apps.codex_usage.proxy, [key]: value } } },
    }))
  }
  const setDisplay = (key: keyof ToolkitConfig['display'], value: number) => {
    setConfig((current) => ({ ...current, display: { ...current.display, [key]: value } }))
  }
  const setBattery = (key: keyof ToolkitConfig['battery'], value: number) => {
    setConfig((current) => ({ ...current, battery: { ...current.battery, [key]: value } }))
  }

  return (
    <div className="min-h-svh text-foreground">
      <aside className="fixed inset-y-0 left-0 z-30 hidden w-[252px] flex-col overflow-hidden border-r border-white/12 bg-ink text-paper lg:flex">
        <div className="flex h-[74px] items-center gap-3 border-b border-white/12 px-5">
          <div className="epd-mark"><span /><span /><span /></div>
          <div>
            <p className="font-display text-lg font-black leading-none tracking-[-.02em]">EPD KIT</p>
            <p className="mt-1 font-mono text-[9px] tracking-[.2em] text-white/46">CONTROL / V1</p>
          </div>
        </div>

        <nav className="flex-1 space-y-1 p-3 pt-6">
          {NAV_ITEMS.map((item, index) => {
            const Icon = item.icon
            const active = view === item.id
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setView(item.id)}
                className={cn('group flex h-11 w-full cursor-pointer items-center gap-3 rounded-md px-3 text-left text-sm font-semibold transition-colors', active ? 'bg-paper text-ink' : 'text-white/58 hover:bg-white/7 hover:text-white')}
              >
                <span className="font-mono text-[9px] opacity-45">0{index + 1}</span>
                <Icon className="size-4" />
                <span>{item.label}</span>
                {active ? <ChevronRight className="ml-auto size-4" /> : null}
              </button>
            )
          })}
        </nav>

        <div className="m-3 rounded-lg border border-white/12 bg-white/4 p-3.5">
          <div className="flex items-center gap-2">
            <span className={cn('size-2 rounded-full', connected ? 'bg-signal shadow-[0_0_10px_#c8ff2f]' : 'bg-white/25')} />
            <span className="truncate font-mono text-[10px] font-bold tracking-[.08em]">{connected ? standardInfo?.name : 'NO DEVICE'}</span>
          </div>
          <p className="mt-2 truncate text-xs text-white/45">{standardInfo?.serial ?? '等待蓝牙连接'}</p>
          {connected ? (
            <button type="button" onClick={disconnect} className="mt-3 flex cursor-pointer items-center gap-1.5 text-[11px] font-semibold text-white/55 hover:text-white">
              <LogOut className="size-3" /> 断开
            </button>
          ) : null}
        </div>
      </aside>

      <div className="lg:pl-[252px]">
        <header className="sticky top-0 z-20 flex h-[66px] items-center justify-between border-b border-border bg-background/88 px-4 backdrop-blur-md sm:px-6 lg:h-[74px] lg:px-8">
          <div className="flex items-center gap-3">
            <div className="epd-mark epd-mark-dark lg:hidden"><span /><span /><span /></div>
            <div>
              <p className="font-mono text-[9px] font-bold tracking-[.16em] text-muted-foreground uppercase lg:hidden">EPD KIT / V1</p>
              <p className="font-display text-base font-black sm:text-lg">{pageName}</p>
            </div>
          </div>
          <div className="flex items-center gap-2.5">
            {connected ? (
              <>
                <div className="hidden items-center gap-2 border-r border-border pr-3 sm:flex">
                  <BatteryCharging className="size-4" />
                  <span className="font-mono text-xs font-bold">{standardInfo?.battery ?? '—'}%</span>
                </div>
                <Badge variant="signal" className="hidden sm:inline-flex"><span className="size-1.5 rounded-full bg-signal-foreground" /> CONNECTED</Badge>
                <Button variant="outline" size="sm" onClick={disconnect}><LogOut /> <span className="hidden sm:inline">断开</span></Button>
              </>
            ) : (
              <Button variant="signal" size="sm" onClick={connect} disabled={phase === 'connecting' || !supportsBluetooth}>
                {phase === 'connecting' ? <LoaderCircle className="animate-spin" /> : <Bluetooth />}
                {phase === 'connecting' ? '连接中' : '连接设备'}
              </Button>
            )}
          </div>
        </header>

        {!supportsBluetooth ? (
          <div className="border-b border-destructive/25 bg-destructive/8 px-4 py-2.5 text-center text-xs font-semibold text-destructive">
            Web Bluetooth 不可用 · 请使用 Chrome / Edge（Android 或桌面）
          </div>
        ) : null}

        <main className="mx-auto max-w-[1240px] px-4 py-6 pb-32 sm:px-6 sm:py-8 lg:px-8 lg:pb-24">
          {view === 'overview' ? (
            <>
              <PageIntro
                eyebrow="01 / Device pulse"
                title={connected ? config.device.name : '等待设备'}
                action={connected ? (
                  <Button variant="outline" size="sm" onClick={reloadDevice} disabled={busy}>
                    <LoadingIcon active={operation === 'reload'} /><RefreshCw /> 同步
                  </Button>
                ) : undefined}
              />

              <div className="grid gap-4 xl:grid-cols-[1.25fr_.75fr]">
                <Panel className="overflow-hidden bg-ink p-0 text-paper">
                  <div className="grid min-h-[350px] items-center gap-7 p-5 sm:p-7 md:grid-cols-[minmax(0,1fr)_210px]">
                    <div className="epd-shell mx-auto w-full max-w-[560px]">
                      <div className="epd-display">
                        <div className="flex items-start justify-between border-b-[3px] border-black pb-2">
                          <div>
                            <p className="font-mono text-[9px] font-black tracking-[.24em]">CODEX USAGE</p>
                            <p className="font-display text-xl font-black leading-tight">{connected ? 'REMAINING' : 'STANDBY'}</p>
                          </div>
                          <div className="text-right font-mono text-[8px] font-bold leading-4">
                            <p>{connected ? 'SYNC READY' : 'BLE OFF'}</p>
                            <p>{new Intl.DateTimeFormat('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date())}</p>
                          </div>
                        </div>
                        <div className="grid grid-cols-2 gap-4 pt-3">
                          <div>
                            <div className="flex items-baseline justify-between"><b className="font-display text-2xl">5H</b><b className="font-mono text-lg">{connected ? '—' : '00'}%</b></div>
                            <div className="mt-2 h-3 border-2 border-black p-[2px]"><div className="h-full w-[68%] bg-black" /></div>
                          </div>
                          <div>
                            <div className="flex items-baseline justify-between"><b className="font-display text-2xl">7D</b><b className="font-mono text-lg">{connected ? '—' : '00'}%</b></div>
                            <div className="mt-2 h-3 border-2 border-black p-[2px]"><div className="h-full w-[42%] bg-black" /></div>
                          </div>
                        </div>
                      </div>
                    </div>
                    <div className="space-y-5 border-t border-white/12 pt-5 md:border-t-0 md:border-l md:pt-0 md:pl-7">
                      <div>
                        <p className="micro-label text-white/38">FIRMWARE</p>
                        <p className="mt-1.5 font-mono text-sm font-bold">{hello?.firmware ?? standardInfo?.firmware ?? '—'}</p>
                      </div>
                      <div>
                        <p className="micro-label text-white/38">PROTOCOL</p>
                        <p className="mt-1.5 font-mono text-sm font-bold">{hello ? `v${hello.protocol} / MTU ${hello.mtu}` : 'v1 / —'}</p>
                      </div>
                      <div>
                        <p className="micro-label text-white/38">RADIO</p>
                        <p className="mt-1.5 flex items-center gap-2 font-mono text-sm font-bold"><Radio className="size-3.5" /> {connected ? `UP ${formatUptime(status?.uptime_ms)}` : 'OFFLINE'}</p>
                      </div>
                    </div>
                  </div>
                </Panel>

                <div className="grid grid-cols-2 gap-4 xl:grid-cols-1">
                  <Panel className="stat-card">
                    <div className="flex items-start justify-between"><BatteryCharging className="size-5" /><span className="micro-label">BATTERY</span></div>
                    <p className="stat-value">{standardInfo?.battery ?? '—'}<small>{standardInfo?.battery !== undefined ? '%' : ''}</small></p>
                    <Progress value={standardInfo?.battery ?? 0} />
                  </Panel>
                  <Panel className="stat-card">
                    <div className="flex items-start justify-between"><Wifi className="size-5" /><span className="micro-label">WI-FI</span></div>
                    <p className="mt-5 truncate font-display text-lg font-black">{status?.wifi_ssid || config.wifi.ssid || '未配置'}</p>
                    <p className="mt-1 font-mono text-[10px] text-muted-foreground">2.4 GHZ / {config.wifi.ipv4.mode.toUpperCase()}</p>
                  </Panel>
                </div>
              </div>

              <div className="mt-4 grid gap-4 lg:grid-cols-[.9fr_1.1fr]">
                <Panel>
                  <PanelTitle icon={Zap} title="快速操作" />
                  <div className="grid gap-2 p-4 sm:grid-cols-2">
                    <Button variant="outline" className="h-12 justify-start" onClick={refreshNow} disabled={!connected || busy}><RefreshCw /> 立即刷新</Button>
                    <Button variant="outline" className="h-12 justify-start" onClick={() => setView('network')}><Wifi /> 配置网络</Button>
                    <Button variant="outline" className="h-12 justify-start" onClick={() => setView('codex')}><KeyRound /> 更新凭据</Button>
                    <Button variant="outline" className="h-12 justify-start" onClick={() => setView('system')}><Settings2 /> 系统参数</Button>
                  </div>
                </Panel>
                <Panel>
                  <PanelTitle icon={Activity} title="会话记录" action={<Badge variant="outline">{activities.length} EVENTS</Badge>} />
                  <div className="divide-y divide-border">
                    {activities.length ? activities.slice(0, 4).map((entry, index) => (
                      <div key={`${entry.at}-${index}`} className="flex items-center gap-3 px-4 py-3 text-xs sm:px-5">
                        <span className={cn('size-2 rounded-full', entry.kind === 'error' ? 'bg-destructive' : entry.kind === 'response' ? 'bg-signal-foreground' : 'bg-foreground/28')} />
                        <span className="min-w-0 flex-1 truncate font-medium">{entry.label}</span>
                        <span className="font-mono text-[10px] text-muted-foreground">{new Date(entry.at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })}</span>
                      </div>
                    )) : (
                      <div className="grid min-h-36 place-items-center p-5 text-center">
                        <div><Circle className="mx-auto mb-2 size-4 text-muted-foreground/45" /><p className="text-xs text-muted-foreground">暂无会话</p></div>
                      </div>
                    )}
                  </div>
                </Panel>
              </div>

            </>
          ) : null}

          {view === 'network' ? (
            <>
              <PageIntro
                eyebrow="02 / Connectivity"
                title="网络配置"
                action={<Button variant="outline" size="sm" onClick={scanWifi} disabled={!connected || busy}><LoadingIcon active={operation === 'wifi.scan'} /><ScanLine /> 扫描</Button>}
              />
              <div className="grid gap-4 lg:grid-cols-[1fr_.72fr]">
                <Panel>
                  <PanelTitle icon={Wifi} title="2.4 GHz Wi-Fi" action={config.wifi.password_set ? <Badge variant="signal"><LockKeyhole /> SAVED</Badge> : <Badge variant="outline">NO SECRET</Badge>} />
                  <div className="space-y-5 p-4 sm:p-5">
                    <Field label="SSID" htmlFor="wifi-ssid">
                      <Input id="wifi-ssid" maxLength={32} value={config.wifi.ssid} onChange={(event) => setConfig((current) => ({ ...current, wifi: { ...current.wifi, ssid: event.target.value } }))} placeholder="选择或输入网络" />
                    </Field>
                    <Field label="密码" htmlFor="wifi-password" hint={config.wifi.password_set && !wifiPassword ? '留空保留已存密码' : undefined}>
                      <div className="relative">
                        <Input id="wifi-password" type={showWifiPassword ? 'text' : 'password'} maxLength={64} value={wifiPassword} onChange={(event) => { setWifiPassword(event.target.value); setClearWifiPassword(false) }} placeholder={config.wifi.password_set ? '••••••••••••' : '开放网络可留空'} className="pr-11" />
                        <button type="button" onClick={() => setShowWifiPassword((value) => !value)} className="absolute top-1/2 right-3 -translate-y-1/2 cursor-pointer text-muted-foreground hover:text-foreground" aria-label="显示或隐藏密码">
                          {showWifiPassword ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                        </button>
                      </div>
                    </Field>
                    <div className="flex items-center justify-between rounded-md border border-border bg-muted/45 px-3.5 py-3">
                      <div><p className="text-xs font-semibold">清除已存密码</p><p className="mt-0.5 text-[10px] text-muted-foreground">发送空字符串</p></div>
                      <Switch checked={clearWifiPassword} onCheckedChange={(checked) => { setClearWifiPassword(checked); if (checked) setWifiPassword('') }} />
                    </div>
                    <div className="grid gap-2 sm:grid-cols-2">
                      <Button variant="outline" onClick={() => patchNetwork(false)} disabled={!connected || busy}><LoadingIcon active={operation === 'wifi.patch'} /><Save /> 暂存</Button>
                      <Button variant="signal" onClick={() => patchNetwork(true)} disabled={!connected || busy}><LoadingIcon active={operation === 'wifi.test'} /><SignalHigh /> 暂存并测试</Button>
                    </div>
                  </div>
                </Panel>

                <Panel>
                  <PanelTitle icon={Radio} title="附近网络" action={<span className="font-mono text-[10px] text-muted-foreground">{networks.length}/10</span>} />
                  <div className="max-h-[430px] divide-y divide-border overflow-y-auto">
                    {networks.length ? networks.map((network) => (
                      <button
                        type="button"
                        key={`${network.ssid}-${network.channel}`}
                        onClick={() => setConfig((current) => ({ ...current, wifi: { ...current.wifi, ssid: network.ssid } }))}
                        className={cn('flex w-full cursor-pointer items-center gap-3 px-4 py-3.5 text-left transition-colors hover:bg-muted/70 sm:px-5', config.wifi.ssid === network.ssid && 'bg-signal/13')}
                      >
                        <div className="flex h-7 w-8 items-end gap-[2px]">
                          {[1, 2, 3, 4].map((bar) => <span key={bar} className={cn('w-1 rounded-t-[1px] bg-foreground/14', bar <= rssiBars(network.rssi) && 'bg-foreground')} style={{ height: `${bar * 4 + 2}px` }} />)}
                        </div>
                        <div className="min-w-0 flex-1"><p className="truncate text-sm font-semibold">{network.ssid || '隐藏网络'}</p><p className="font-mono text-[9px] text-muted-foreground">CH {network.channel} · {network.rssi} DBM</p></div>
                        {network.open ? <WifiOff className="size-3.5 text-muted-foreground" /> : <LockKeyhole className="size-3.5" />}
                      </button>
                    )) : (
                      <div className="grid min-h-64 place-items-center p-6 text-center"><div><ScanLine className="mx-auto mb-3 size-6 text-muted-foreground/40" /><p className="text-xs font-semibold text-muted-foreground">连接后扫描</p></div></div>
                    )}
                  </div>
                </Panel>
              </div>

              <Panel className="mt-4">
                <PanelTitle
                  icon={Network}
                  title="IPv4"
                  action={wifiTestResult ? <Badge variant="signal"><Check /> {wifiTestResult.ipv4_mode.toUpperCase()}</Badge> : undefined}
                />
                <div className="p-4 sm:p-5">
                  <div className="mb-5 grid grid-cols-2 gap-1 rounded-md border border-border bg-muted p-1 sm:w-72">
                    {(['dhcp', 'static'] as const).map((mode) => (
                      <button
                        key={mode}
                        type="button"
                        onClick={() => setIpv4('mode', mode)}
                        className={cn('h-8 cursor-pointer rounded-sm font-mono text-[10px] font-bold tracking-[.12em] uppercase transition-colors', config.wifi.ipv4.mode === mode ? 'bg-foreground text-background shadow-sm' : 'text-muted-foreground hover:text-foreground')}
                      >
                        {mode === 'dhcp' ? '自动 / DHCP' : '手动 / STATIC'}
                      </button>
                    ))}
                  </div>
                  <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-5">
                    <Field label="IP 地址" htmlFor="ipv4-address"><Input id="ipv4-address" inputMode="decimal" value={config.wifi.ipv4.address} onChange={(event) => setIpv4('address', event.target.value)} placeholder="192.168.1.42" disabled={config.wifi.ipv4.mode === 'dhcp'} /></Field>
                    <Field label="网关" htmlFor="ipv4-gateway"><Input id="ipv4-gateway" inputMode="decimal" value={config.wifi.ipv4.gateway} onChange={(event) => setIpv4('gateway', event.target.value)} placeholder="192.168.1.1" disabled={config.wifi.ipv4.mode === 'dhcp'} /></Field>
                    <Field label="子网掩码" htmlFor="ipv4-subnet"><Input id="ipv4-subnet" inputMode="decimal" value={config.wifi.ipv4.subnet} onChange={(event) => setIpv4('subnet', event.target.value)} placeholder="255.255.255.0" disabled={config.wifi.ipv4.mode === 'dhcp'} /></Field>
                    <Field label="DNS 1" htmlFor="ipv4-dns1"><Input id="ipv4-dns1" inputMode="decimal" value={config.wifi.ipv4.dns1} onChange={(event) => setIpv4('dns1', event.target.value)} placeholder="1.1.1.1" disabled={config.wifi.ipv4.mode === 'dhcp'} /></Field>
                    <Field label="DNS 2 / 可选" htmlFor="ipv4-dns2"><Input id="ipv4-dns2" inputMode="decimal" value={config.wifi.ipv4.dns2} onChange={(event) => setIpv4('dns2', event.target.value)} placeholder="8.8.8.8" disabled={config.wifi.ipv4.mode === 'dhcp'} /></Field>
                  </div>
                  {wifiTestResult ? (
                    <div className="mt-5 grid gap-3 rounded-md border border-signal-foreground/20 bg-signal/12 p-3 sm:grid-cols-3 xl:grid-cols-6">
                      {[
                        ['IP', wifiTestResult.ip],
                        ['GATEWAY', wifiTestResult.gateway],
                        ['SUBNET', wifiTestResult.subnet],
                        ['DNS 1', wifiTestResult.dns1],
                        ['DNS 2', wifiTestResult.dns2 || '—'],
                        ['RSSI', `${wifiTestResult.rssi} dBm`],
                      ].map(([label, value]) => <div key={label}><p className="micro-label">{label}</p><p className="mt-1 truncate font-mono text-[11px] font-bold">{value}</p></div>)}
                    </div>
                  ) : null}
                </div>
              </Panel>
            </>
          ) : null}

          {view === 'codex' ? (
            <>
              <PageIntro eyebrow="03 / Application" title="Codex 应用" />
              <div className="grid gap-4 lg:grid-cols-[1fr_.72fr]">
                <Panel>
                  <PanelTitle icon={KeyRound} title="访问凭据" action={config.apps.codex_usage.access_token_set ? <Badge variant="signal"><ShieldCheck /> TOKEN SET</Badge> : <Badge variant="destructive">TOKEN EMPTY</Badge>} />
                  <div className="space-y-5 p-4 sm:p-5">
                    <Field label="Account ID" htmlFor="account-id">
                      <Input id="account-id" maxLength={128} value={config.apps.codex_usage.account_id} onChange={(event) => setConfig((current) => ({ ...current, apps: { codex_usage: { ...current.apps.codex_usage, account_id: event.target.value } } }))} placeholder="account-id" />
                    </Field>
                    <Field label="Access Token" htmlFor="access-token" hint={config.apps.codex_usage.access_token_set && !accessToken ? '留空保留已存 Token' : '最多 4096 bytes'}>
                      <div className="relative">
                        <Input id="access-token" type={showAccessToken ? 'text' : 'password'} value={accessToken} onChange={(event) => { setAccessToken(event.target.value); setClearAccessToken(false) }} placeholder={config.apps.codex_usage.access_token_set ? '••••••••••••' : 'eyJ...'} className="pr-11 font-mono text-xs" />
                        <button type="button" onClick={() => setShowAccessToken((value) => !value)} className="absolute top-1/2 right-3 -translate-y-1/2 cursor-pointer text-muted-foreground hover:text-foreground" aria-label="显示或隐藏 Token">
                          {showAccessToken ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                        </button>
                      </div>
                    </Field>
                    <Field label="到期时间" htmlFor="expires-at">
                      <Input id="expires-at" type="datetime-local" value={toDateTimeInput(config.apps.codex_usage.expires_at)} onChange={(event) => setConfig((current) => ({ ...current, apps: { codex_usage: { ...current.apps.codex_usage, expires_at: fromDateTimeInput(event.target.value) } } }))} />
                    </Field>
                    <div className="flex items-center justify-between rounded-md border border-border bg-muted/45 px-3.5 py-3">
                      <div><p className="text-xs font-semibold">清除已存 Token</p><p className="mt-0.5 text-[10px] text-muted-foreground">发送空字符串</p></div>
                      <Switch checked={clearAccessToken} onCheckedChange={(checked) => { setClearAccessToken(checked); if (checked) setAccessToken('') }} />
                    </div>
                    <Button variant="signal" className="w-full" onClick={patchCodex} disabled={!connected || busy}><LoadingIcon active={operation === 'codex.patch'} /><Save /> 暂存凭据</Button>
                  </div>
                </Panel>

                <div className="space-y-4">
                  <Panel className="overflow-hidden">
                    <div className="codex-card p-5">
                      <div className="flex items-start justify-between">
                        <div className="grid size-11 place-items-center rounded-lg bg-ink text-paper"><AppWindow className="size-5" /></div>
                        <Badge variant={apps.some((app) => app.id === 'codex_usage' && app.active) || config.active_app === 'codex_usage' ? 'signal' : 'outline'}>ACTIVE</Badge>
                      </div>
                      <h2 className="mt-8 font-display text-2xl font-black">Codex Usage</h2>
                      <p className="mt-1 font-mono text-[10px] tracking-[.14em] text-muted-foreground">{apps.find((app) => app.id === 'codex_usage')?.version ?? 'V1'} / STATIC APP</p>
                      <div className="mt-6 grid grid-cols-2 gap-2 border-t border-border pt-4">
                        <div><p className="micro-label">ACCOUNT</p><p className="mt-1 truncate text-xs font-semibold">{config.apps.codex_usage.account_id || '—'}</p></div>
                        <div><p className="micro-label">EXPIRES</p><p className="mt-1 text-xs font-semibold">{formatExpiry(config.apps.codex_usage.expires_at)}</p></div>
                      </div>
                    </div>
                  </Panel>
                  <Button variant="outline" className="w-full" onClick={activateCodex} disabled={!connected || busy || config.active_app === 'codex_usage'}><Check /> 设为当前应用</Button>
                  <Button variant="default" className="w-full" onClick={refreshNow} disabled={!connected || busy}><LoadingIcon active={operation === 'refresh.now'} /><RefreshCw /> 立即刷新屏幕</Button>
                </div>
              </div>

              <Panel className="mt-4">
                <PanelTitle
                  icon={Server}
                  title="HTTP CONNECT 代理"
                  action={config.apps.codex_usage.proxy.enabled ? <Badge variant="signal">ENABLED</Badge> : <Badge variant="outline">OFF</Badge>}
                />
                <div className="space-y-5 p-4 sm:p-5">
                  <div className="flex items-center justify-between rounded-md border border-border bg-muted/45 px-3.5 py-3">
                    <div><p className="text-xs font-semibold">使用代理</p><p className="mt-0.5 font-mono text-[9px] text-muted-foreground">CHATGPT.COM:443</p></div>
                    <Switch checked={config.apps.codex_usage.proxy.enabled} onCheckedChange={(checked) => setProxy('enabled', checked)} />
                  </div>
                  <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                    <Field label="主机" htmlFor="proxy-host"><Input id="proxy-host" maxLength={253} value={config.apps.codex_usage.proxy.host} onChange={(event) => setProxy('host', event.target.value)} placeholder="proxy.lan" disabled={!config.apps.codex_usage.proxy.enabled} /></Field>
                    <Field label="端口" htmlFor="proxy-port"><Input id="proxy-port" type="number" min={1} max={65535} value={config.apps.codex_usage.proxy.port} onChange={(event) => setProxy('port', Number(event.target.value))} disabled={!config.apps.codex_usage.proxy.enabled} /></Field>
                    <Field label="用户名 / 可选" htmlFor="proxy-username"><Input id="proxy-username" maxLength={128} value={config.apps.codex_usage.proxy.username} onChange={(event) => { setProxy('username', event.target.value); if (!event.target.value) { setProxyPassword(''); setClearProxyPassword(true) } }} placeholder="epd-kit" disabled={!config.apps.codex_usage.proxy.enabled} /></Field>
                    <Field label="密码 / 可选" htmlFor="proxy-password" hint={config.apps.codex_usage.proxy.password_set && !proxyPassword ? '留空保留已存密码' : undefined}>
                      <div className="relative">
                        <Input id="proxy-password" type={showProxyPassword ? 'text' : 'password'} maxLength={256} value={proxyPassword} onChange={(event) => { setProxyPassword(event.target.value); setClearProxyPassword(false) }} placeholder={config.apps.codex_usage.proxy.password_set ? '••••••••••••' : '无需认证可留空'} className="pr-11" disabled={!config.apps.codex_usage.proxy.enabled || !config.apps.codex_usage.proxy.username} />
                        <button type="button" onClick={() => setShowProxyPassword((value) => !value)} disabled={!config.apps.codex_usage.proxy.enabled || !config.apps.codex_usage.proxy.username} className="absolute top-1/2 right-3 -translate-y-1/2 cursor-pointer text-muted-foreground hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40" aria-label="显示或隐藏代理密码">
                          {showProxyPassword ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                        </button>
                      </div>
                    </Field>
                  </div>
                  {config.apps.codex_usage.proxy.username ? (
                    <div className="flex items-center justify-between rounded-md border border-border bg-muted/45 px-3.5 py-3 sm:max-w-sm">
                      <div><p className="text-xs font-semibold">清除代理密码</p><p className="mt-0.5 text-[10px] text-muted-foreground">发送空字符串</p></div>
                      <Switch checked={clearProxyPassword} onCheckedChange={(checked) => { setClearProxyPassword(checked); if (checked) setProxyPassword('') }} disabled={!config.apps.codex_usage.proxy.enabled} />
                    </div>
                  ) : null}
                  <Button variant="signal" className="w-full sm:w-auto" onClick={patchCodex} disabled={!connected || busy}><LoadingIcon active={operation === 'codex.patch'} /><Save /> 暂存 Codex 配置</Button>
                </div>
              </Panel>
            </>
          ) : null}

          {view === 'system' ? (
            <>
              <PageIntro eyebrow="04 / Runtime" title="系统参数" />
              <div className="grid gap-4 lg:grid-cols-2">
                <Panel>
                  <PanelTitle icon={Cpu} title="设备" />
                  <div className="field-grid p-4 sm:p-5">
                    <Field label="名称" htmlFor="device-name"><Input id="device-name" maxLength={24} value={config.device.name} onChange={(event) => setConfig((current) => ({ ...current, device: { ...current.device, name: event.target.value } }))} /></Field>
                    <Field label="Locale" htmlFor="locale"><Input id="locale" maxLength={16} value={config.device.locale} onChange={(event) => setConfig((current) => ({ ...current, device: { ...current.device, locale: event.target.value } }))} /></Field>
                    <Field label="IANA 时区" htmlFor="iana"><Input id="iana" maxLength={64} value={config.device.timezone.iana} onChange={(event) => setConfig((current) => ({ ...current, device: { ...current.device, timezone: { ...current.device.timezone, iana: event.target.value } } }))} /></Field>
                    <Field label="POSIX TZ" htmlFor="posix"><Input id="posix" maxLength={96} value={config.device.timezone.posix} onChange={(event) => setConfig((current) => ({ ...current, device: { ...current.device, timezone: { ...current.device.timezone, posix: event.target.value } } }))} /></Field>
                  </div>
                </Panel>

                <Panel>
                  <PanelTitle icon={Clock3} title="功耗调度" />
                  <div className="field-grid p-4 sm:p-5">
                    <Field label="轮询间隔 / 秒" htmlFor="poll"><Input id="poll" type="number" min={60} max={86400} value={config.power.poll_interval_sec} onChange={(event) => setPower('poll_interval_sec', Number(event.target.value))} /></Field>
                    <Field label="BLE 窗口 / 秒" htmlFor="ble-window"><Input id="ble-window" type="number" min={30} max={600} value={config.power.ble_window_sec} onChange={(event) => setPower('ble_window_sec', Number(event.target.value))} /></Field>
                    {config.power.offline_backoff_sec.map((value, index) => (
                      <Field key={index} label={`离线退避 ${index + 1} / 秒`} htmlFor={`backoff-${index}`}>
                        <Input id={`backoff-${index}`} type="number" min={60} max={86400} value={value} onChange={(event) => {
                          const next = [...config.power.offline_backoff_sec] as [number, number, number, number]
                          next[index] = Number(event.target.value)
                          setPower('offline_backoff_sec', next)
                        }} />
                      </Field>
                    ))}
                  </div>
                </Panel>

                <Panel>
                  <PanelTitle icon={Gauge} title="刷新策略" />
                  <div className="field-grid p-4 sm:p-5">
                    <Field label="局刷次数上限" htmlFor="partial-count"><Input id="partial-count" type="number" min={1} max={100} value={config.display.full_after_partial_count} onChange={(event) => setDisplay('full_after_partial_count', Number(event.target.value))} /></Field>
                    <Field label="全刷最大间隔 / 秒" htmlFor="full-age"><Input id="full-age" type="number" min={3600} value={config.display.full_max_age_sec} onChange={(event) => setDisplay('full_max_age_sec', Number(event.target.value))} /></Field>
                    <Field label="全刷面积阈值 / %" htmlFor="area-threshold"><Input id="area-threshold" type="number" min={10} max={100} value={config.display.full_area_threshold_percent} onChange={(event) => setDisplay('full_area_threshold_percent', Number(event.target.value))} /></Field>
                  </div>
                </Panel>

                <Panel>
                  <PanelTitle icon={BatteryCharging} title="电池阈值" />
                  <div className="field-grid p-4 sm:p-5">
                    <Field label="临界 / mV" htmlFor="critical-mv"><Input id="critical-mv" type="number" min={3000} max={4300} value={config.battery.critical_mv} onChange={(event) => setBattery('critical_mv', Number(event.target.value))} /></Field>
                    <Field label="低电量 / mV" htmlFor="low-mv"><Input id="low-mv" type="number" min={3000} max={4300} value={config.battery.low_mv} onChange={(event) => setBattery('low_mv', Number(event.target.value))} /></Field>
                    <Field label="恢复 / mV" htmlFor="recovery-mv"><Input id="recovery-mv" type="number" min={3000} max={4300} value={config.battery.recovery_mv} onChange={(event) => setBattery('recovery_mv', Number(event.target.value))} /></Field>
                  </div>
                </Panel>
              </div>

              <div className="mt-4 grid gap-4 lg:grid-cols-[1fr_auto]">
                <Button variant="signal" size="lg" onClick={patchSystem} disabled={!connected || busy}><LoadingIcon active={operation === 'system.patch'} /><Save /> 暂存系统参数</Button>
                <Dialog open={resetOpen} onOpenChange={(open) => { setResetOpen(open); if (!open) setResetNonce(null) }}>
                  <DialogTrigger asChild>
                    <Button variant="outline" size="lg" disabled={!connected || busy}><Trash2 /> 恢复出厂</Button>
                  </DialogTrigger>
                  <DialogContent>
                    <DialogHeader>
                      <DialogTitle>恢复出厂</DialogTitle>
                      <DialogDescription>清除配置、额度快照和全部 BLE bond。</DialogDescription>
                    </DialogHeader>
                    {resetNonce ? (
                      <div className="rounded-lg border border-destructive/25 bg-destructive/7 p-4">
                        <p className="micro-label text-destructive">PHYSICAL CONFIRMATION</p>
                        <p className="mt-2 font-display text-lg font-black">按住设备 KEY 2 秒</p>
                        <p className="mt-2 font-mono text-xs">NONCE {resetNonce} · {resetSeconds}s</p>
                      </div>
                    ) : (
                      <div className="flex items-center gap-3 rounded-lg border border-border bg-muted/55 p-4 text-sm font-semibold"><RotateCcw className="size-5" /> 先生成 30 秒确认码</div>
                    )}
                    <DialogFooter>
                      <DialogClose asChild><Button variant="outline">取消</Button></DialogClose>
                      {resetNonce ? (
                        <Button variant="destructive" onClick={commitReset} disabled={busy || resetSeconds <= 0}><LoadingIcon active={operation === 'factory.commit'} />确认擦除</Button>
                      ) : (
                        <Button variant="destructive" onClick={prepareReset} disabled={busy}><LoadingIcon active={operation === 'factory.prepare'} />生成确认码</Button>
                      )}
                    </DialogFooter>
                  </DialogContent>
                </Dialog>
              </div>
            </>
          ) : null}
        </main>
      </div>

      {staged ? (
        <div className="fixed right-3 bottom-[76px] left-3 z-40 flex items-center justify-between gap-3 rounded-lg border border-signal-foreground/25 bg-ink px-4 py-3 text-paper shadow-[5px_5px_0_rgba(0,0,0,.18)] sm:right-5 sm:left-auto sm:min-w-[380px] lg:bottom-5">
          <div className="flex items-center gap-2.5"><span className="size-2 rounded-full bg-signal" /><div><p className="text-xs font-bold">有未提交更改</p><p className="font-mono text-[9px] text-white/46">STAGING / RAM</p></div></div>
          <Button variant="signal" size="sm" onClick={commitConfig} disabled={busy}><LoadingIcon active={operation === 'config.commit'} /><Save /> 写入设备</Button>
        </div>
      ) : null}

      <nav className="fixed right-0 bottom-0 left-0 z-30 grid h-[66px] grid-cols-4 border-t border-border bg-background/94 px-1 pb-[env(safe-area-inset-bottom)] backdrop-blur-md lg:hidden">
        {NAV_ITEMS.map((item) => {
          const Icon = item.icon
          const active = view === item.id
          return (
            <button key={item.id} type="button" onClick={() => setView(item.id)} className={cn('relative flex cursor-pointer flex-col items-center justify-center gap-1 text-[9px] font-bold transition-colors', active ? 'text-foreground' : 'text-muted-foreground')}>
              {active ? <span className="absolute top-0 h-[3px] w-8 rounded-b-full bg-foreground" /> : null}
              <Icon className="size-4" />
              <span>{item.shortLabel}</span>
            </button>
          )
        })}
      </nav>

      <Toaster position="top-center" richColors closeButton toastOptions={{ className: 'font-sans text-sm' }} />
    </div>
  )
}

export default App
