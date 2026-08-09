export const BLE_UUIDS = {
  toolkitService: 'f0a10000-0451-4000-b000-000000000001',
  rx: 'f0a10001-0451-4000-b000-000000000001',
  tx: 'f0a10002-0451-4000-b000-000000000001',
  deviceInformation: 0x180a,
  manufacturer: 0x2a29,
  model: 0x2a24,
  firmware: 0x2a26,
  serial: 0x2a25,
  batteryService: 0x180f,
  batteryLevel: 0x2a19,
} as const

export type ToolkitConfig = {
  version: number
  device: {
    name: string
    locale: string
    timezone: { iana: string; posix: string }
  }
  wifi: {
    ssid: string
    password?: string
    password_set?: boolean
    ipv4: {
      mode: 'dhcp' | 'static'
      address: string
      gateway: string
      subnet: string
      dns1: string
      dns2: string
    }
  }
  power: {
    poll_interval_sec: number
    ble_window_sec: number
    offline_backoff_sec: [number, number, number, number]
  }
  display: {
    full_after_partial_count: number
    full_max_age_sec: number
    full_area_threshold_percent: number
  }
  battery: {
    low_mv: number
    critical_mv: number
    recovery_mv: number
  }
  active_app: 'codex_usage'
  apps: {
    codex_usage: {
      account_id: string
      access_token?: string
      access_token_set?: boolean
      expires_at: number
      proxy: {
        enabled: boolean
        host: string
        port: number
        username: string
        password?: string
        password_set?: boolean
      }
    }
  }
}

export type HelloResult = {
  protocol: number
  firmware: string
  device_name: string
  max_message_bytes: number
  mtu: number
  security: string | Record<string, unknown>
}

export type DeviceStatus = {
  configured: boolean
  config_committed: boolean
  active_app: string
  uptime_ms: number
  wifi_ssid: string
}

export type WifiNetwork = {
  ssid: string
  rssi: number
  channel: number
  open: boolean
}

export type WifiTestResult = {
  connected: boolean
  rssi: number
  ipv4_mode: 'dhcp' | 'static'
  ip: string
  gateway: string
  subnet: string
  dns1: string
  dns2: string
}

export type ToolkitApp = {
  id: string
  name: string
  version: string
  active: boolean
}

export type StandardDeviceInfo = {
  name: string
  manufacturer?: string
  model?: string
  firmware?: string
  serial?: string
  battery?: number
}

export type BleActivity = {
  at: number
  kind: 'request' | 'response' | 'system' | 'error'
  label: string
}

type ToolkitResponse = {
  v: number
  id: number
  ok: boolean
  result?: unknown
  error?: { code: string; message: string }
}

type PendingRequest = {
  id: number
  op: string
  startedAt: number
  resolve: (value: unknown) => void
  reject: (reason: Error) => void
  timer: ReturnType<typeof setTimeout>
}

export class ToolkitError extends Error {
  readonly code: string

  constructor(code: string, message: string) {
    super(message)
    this.name = 'ToolkitError'
    this.code = code
  }
}

const encoder = new TextEncoder()
const decoder = new TextDecoder('utf-8', { fatal: true })
const LOG_PREFIX = '[EPD BLE]'

function debugLog(event: string, details: Record<string, string | number | boolean | undefined> = {}) {
  console.info(LOG_PREFIX, event, details)
}

function warningLog(event: string, details: Record<string, string | number | boolean | undefined> = {}) {
  console.warn(LOG_PREFIX, event, details)
}

function concatBytes(left: Uint8Array, right: Uint8Array) {
  const joined = new Uint8Array(left.length + right.length)
  joined.set(left)
  joined.set(right, left.length)
  return joined
}

function delay(ms: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, ms))
}

export class ToolkitBleClient {
  onActivity?: (activity: BleActivity) => void
  onDisconnected?: () => void
  onBattery?: (level: number) => void

  private device?: BluetoothDevice
  private rx?: BluetoothRemoteGATTCharacteristic
  private tx?: BluetoothRemoteGATTCharacteristic
  private battery?: BluetoothRemoteGATTCharacteristic
  private pending?: PendingRequest
  private incoming = new Uint8Array()
  private nextId = (Date.now() >>> 0) || 1
  private manuallyDisconnecting = false

  get connected() {
    return Boolean(this.device?.gatt?.connected && this.rx && this.tx)
  }

  async connect(): Promise<StandardDeviceInfo> {
    if (!navigator.bluetooth) {
      throw new ToolkitError('unsupported_browser', '当前浏览器不支持 Web Bluetooth')
    }

    this.manuallyDisconnecting = false
    this.emit('system', '选择设备')
    debugLog('device.request', { service: BLE_UUIDS.toolkitService })
    const device = await navigator.bluetooth.requestDevice({
      filters: [{ services: [BLE_UUIDS.toolkitService] }],
      optionalServices: [BLE_UUIDS.deviceInformation, BLE_UUIDS.batteryService],
    })
    const gatt = device.gatt
    if (!gatt) throw new ToolkitError('gatt_unavailable', '设备未提供 GATT')

    this.device = device
    debugLog('device.selected', { name: device.name ?? 'EPD-KIT', id: device.id })
    device.addEventListener('gattserverdisconnected', this.handleDisconnect)
    const server = await gatt.connect()
    debugLog('gatt.connected')
    const service = await server.getPrimaryService(BLE_UUIDS.toolkitService)
    debugLog('gatt.service.ready', { service: BLE_UUIDS.toolkitService })
    this.rx = await service.getCharacteristic(BLE_UUIDS.rx)
    this.tx = await service.getCharacteristic(BLE_UUIDS.tx)
    this.tx.addEventListener('characteristicvaluechanged', this.handleIndication)
    await this.tx.startNotifications()
    debugLog('gatt.tx.subscribed')
    this.emit('system', `已连接 ${device.name ?? 'EPD-KIT'}`)

    const info: StandardDeviceInfo = { name: device.name ?? 'EPD-KIT' }
    // Chromium serializes GATT operations per connection. Keep optional service
    // discovery, reads and notification setup strictly ordered as well.
    await this.readDeviceInformation(server, info)
    await this.setupBattery(server, info)
    debugLog('device.ready', {
      firmwareAvailable: Boolean(info.firmware),
      batteryAvailable: info.battery !== undefined,
    })
    return info
  }

  disconnect() {
    debugLog('gatt.disconnect.request')
    this.manuallyDisconnecting = true
    this.rejectPending(new ToolkitError('disconnected', '连接已断开'))
    this.teardownCharacteristics()
    if (this.device?.gatt?.connected) this.device.gatt.disconnect()
    this.device = undefined
    this.incoming = new Uint8Array()
  }

  async transact<T>(op: string, args: Record<string, unknown> = {}, timeoutMs?: number): Promise<T> {
    const timeout = timeoutMs ?? (op === 'wifi.scan' || op === 'wifi.test' ? 20_000 : 15_000)
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        return await this.requestOnce<T>(op, args, timeout)
      } catch (error) {
        const retryable = error instanceof ToolkitError && (error.code === 'busy' || error.code === 'timeout')
        if (!retryable || attempt === 1) throw error
        warningLog('request.retry', { op, attempt: attempt + 1, code: error.code })
        this.emit('system', `${error.code} · 重试`)
        await delay(error.code === 'busy' ? 450 : 100)
      }
    }
    throw new ToolkitError('internal_error', '请求未完成')
  }

  private async requestOnce<T>(op: string, args: Record<string, unknown>, timeoutMs: number): Promise<T> {
    if (!this.connected || !this.rx) throw new ToolkitError('disconnected', '请先连接设备')
    if (this.pending) throw new ToolkitError('busy', '已有请求处理中')

    this.nextId = (this.nextId + 1) >>> 0
    if (this.nextId === 0) this.nextId = 1
    const id = this.nextId
    const payload = JSON.stringify({ v: 1, id, op, args })
    const body = encoder.encode(payload)
    if (body.byteLength > 8192) throw new ToolkitError('too_large', '请求超过 8192 bytes')
    const wire = encoder.encode(`${payload}\n`)
    debugLog('request.start', {
      id,
      op,
      bytes: body.byteLength,
      fragments: Math.ceil(wire.byteLength / 20),
      timeoutMs,
    })
    this.emit('request', op)

    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        if (this.pending?.id !== id) return
        this.pending = undefined
        const error = new ToolkitError('client_timeout', `${op} 响应超时`)
        this.emit('error', `${op} · 响应超时`)
        reject(error)
      }, timeoutMs)

      this.pending = {
        id,
        op,
        startedAt: performance.now(),
        timer,
        resolve: (value) => resolve(value as T),
        reject,
      }

      void this.writeFragments(wire).catch((error: unknown) => {
        if (this.pending?.id !== id) return
        this.rejectPending(error instanceof Error ? error : new Error(String(error)))
      })
    })
  }

  private async writeFragments(wire: Uint8Array) {
    if (!this.rx) throw new ToolkitError('disconnected', 'RX 不可用')
    const chunkSize = 20
    const fragments = Math.ceil(wire.byteLength / chunkSize)
    for (let offset = 0; offset < wire.byteLength; offset += chunkSize) {
      const chunk = wire.slice(offset, offset + chunkSize)
      await this.rx.writeValueWithResponse(chunk)
    }
    debugLog('request.written', { bytes: wire.byteLength, fragments })
  }

  private handleIndication = (event: Event) => {
    const characteristic = event.target as BluetoothRemoteGATTCharacteristic
    const value = characteristic.value
    if (!value) return
    const fragment = new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
    debugLog('indication.received', { bytes: fragment.byteLength })
    this.incoming = concatBytes(this.incoming, fragment)

    let newline = this.incoming.indexOf(0x0a)
    while (newline >= 0) {
      let line = this.incoming.slice(0, newline)
      this.incoming = this.incoming.slice(newline + 1)
      if (line.at(-1) === 0x0d) line = line.slice(0, -1)
      this.consumeLine(line)
      newline = this.incoming.indexOf(0x0a)
    }

    if (this.incoming.byteLength > 8192) {
      this.incoming = new Uint8Array()
      warningLog('response.too_large')
      this.rejectPending(new ToolkitError('too_large', '响应超过 8192 bytes'))
    }
  }

  private consumeLine(line: Uint8Array) {
    let response: ToolkitResponse
    try {
      response = JSON.parse(decoder.decode(line)) as ToolkitResponse
    } catch {
      warningLog('response.invalid_json', { bytes: line.byteLength })
      this.emit('error', '收到无效 JSON')
      return
    }

    const pending = this.pending
    if (!pending || (response.id !== pending.id && response.id !== 0)) return
    window.clearTimeout(pending.timer)
    this.pending = undefined

    if (response.v !== 1) {
      warningLog('response.version_mismatch', { id: response.id, version: response.v })
      pending.reject(new ToolkitError('unsupported_version', `协议版本 ${response.v}`))
      return
    }
    if (!response.ok) {
      const code = response.error?.code ?? 'internal_error'
      const message = response.error?.message ?? '设备返回错误'
      warningLog('response.error', {
        id: response.id,
        op: pending.op,
        code,
        elapsedMs: Math.round(performance.now() - pending.startedAt),
      })
      this.emit('error', `${code} · ${message}`)
      pending.reject(new ToolkitError(code, message))
      return
    }

    debugLog('response.ok', {
      id: response.id,
      op: pending.op,
      elapsedMs: Math.round(performance.now() - pending.startedAt),
    })
    this.emit('response', `完成 #${response.id}`)
    pending.resolve(response.result ?? {})
  }

  private rejectPending(error: Error) {
    if (!this.pending) return
    const pending = this.pending
    window.clearTimeout(pending.timer)
    this.pending = undefined
    warningLog('request.failed', {
      id: pending.id,
      op: pending.op,
      name: error.name,
      elapsedMs: Math.round(performance.now() - pending.startedAt),
    })
    pending.reject(error)
  }

  private handleDisconnect = () => {
    debugLog('gatt.disconnected', { manual: this.manuallyDisconnecting })
    this.rejectPending(new ToolkitError('disconnected', '设备已断开'))
    this.teardownCharacteristics()
    this.device = undefined
    this.incoming = new Uint8Array()
    if (!this.manuallyDisconnecting) {
      this.emit('system', '设备已断开')
      this.onDisconnected?.()
    }
    this.manuallyDisconnecting = false
  }

  private teardownCharacteristics() {
    this.tx?.removeEventListener('characteristicvaluechanged', this.handleIndication)
    this.battery?.removeEventListener('characteristicvaluechanged', this.handleBattery)
    this.rx = undefined
    this.tx = undefined
    this.battery = undefined
  }

  private handleBattery = (event: Event) => {
    const characteristic = event.target as BluetoothRemoteGATTCharacteristic
    const level = characteristic.value?.getUint8(0)
    if (level !== undefined) {
      debugLog('battery.changed', { level })
      this.onBattery?.(level)
    }
  }

  private async readDeviceInformation(server: BluetoothRemoteGATTServer, info: StandardDeviceInfo) {
    try {
      const service = await server.getPrimaryService(BLE_UUIDS.deviceInformation)
      const entries: Array<['manufacturer' | 'model' | 'firmware' | 'serial', number]> = [
        ['manufacturer', BLE_UUIDS.manufacturer],
        ['model', BLE_UUIDS.model],
        ['firmware', BLE_UUIDS.firmware],
        ['serial', BLE_UUIDS.serial],
      ]
      for (const [key, uuid] of entries) {
        try {
          const characteristic = await service.getCharacteristic(uuid)
          const value = await characteristic.readValue()
          info[key] = decoder.decode(new Uint8Array(value.buffer, value.byteOffset, value.byteLength))
          debugLog('device_info.read', { characteristic: key, bytes: value.byteLength })
        } catch {
          // Optional characteristic.
        }
      }
    } catch {
      debugLog('device_info.unavailable')
      // Optional service.
    }
  }

  private async setupBattery(server: BluetoothRemoteGATTServer, info: StandardDeviceInfo) {
    try {
      const service = await server.getPrimaryService(BLE_UUIDS.batteryService)
      this.battery = await service.getCharacteristic(BLE_UUIDS.batteryLevel)
      const value = await this.battery.readValue()
      info.battery = value.getUint8(0)
      debugLog('battery.read', { level: info.battery })
      this.battery.addEventListener('characteristicvaluechanged', this.handleBattery)
      await this.battery.startNotifications()
      debugLog('battery.subscribed')
    } catch {
      debugLog('battery.unavailable')
      // Optional service.
    }
  }

  private emit(kind: BleActivity['kind'], label: string) {
    this.onActivity?.({ at: Date.now(), kind, label })
  }
}
