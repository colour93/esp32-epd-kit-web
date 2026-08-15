import { BatteryCharging, Save, Settings2, Zap } from 'lucide-react'
import type { Dispatch, SetStateAction } from 'react'
import type { DeviceConfig } from '@/lib/agent'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Field, FieldGroup, FieldLabel, FieldTitle } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { NumberField } from '@/components/dashboard/dashboard-components'

export const HardwareView = ({ config, setConfig, owner, busy, onSave }: {
  config: DeviceConfig
  setConfig: Dispatch<SetStateAction<DeviceConfig | null>>
  owner: boolean
  busy: boolean
  onSave: () => void
}) => (
  <>
    <div className="flex justify-end"><Button disabled={!owner || busy} onClick={onSave}><Save data-icon="inline-start" />保存</Button></div>
    <Card>
      <CardHeader><CardTitle className="flex items-center gap-2"><BatteryCharging className="size-4" />电池与 IO</CardTitle></CardHeader>
      <CardContent>
        <FieldGroup>
          <Field orientation="horizontal"><FieldTitle>电池输入</FieldTitle><Switch checked={config.hardware.battery.enabled} onCheckedChange={(enabled) => setConfig({ ...config, hardware: { ...config.hardware, battery: { ...config.hardware.battery, enabled } } })} /></Field>
          <div className="grid gap-4 md:grid-cols-3">
            <NumberField label="低电量 / mV" min={3001} max={4298} value={config.hardware.battery.low_mv} disabled={!config.hardware.battery.enabled} onChange={(low_mv) => setConfig({ ...config, hardware: { ...config.hardware, battery: { ...config.hardware.battery, low_mv } } })} />
            <NumberField label="临界电量 / mV" min={3000} max={4297} value={config.hardware.battery.critical_mv} disabled={!config.hardware.battery.enabled} onChange={(critical_mv) => setConfig({ ...config, hardware: { ...config.hardware, battery: { ...config.hardware.battery, critical_mv } } })} />
            <NumberField label="恢复电量 / mV" min={3002} max={4300} value={config.hardware.battery.recovery_mv} disabled={!config.hardware.battery.enabled} onChange={(recovery_mv) => setConfig({ ...config, hardware: { ...config.hardware, battery: { ...config.hardware.battery, recovery_mv } } })} />
          </div>
          <Field orientation="horizontal"><FieldTitle>IO12 按键</FieldTitle><Switch checked={config.hardware.io12.mode === 'key'} onCheckedChange={(enabled) => setConfig({ ...config, hardware: { ...config.hardware, io12: { mode: enabled ? 'key' : 'disabled' } } })} /></Field>
        </FieldGroup>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle className="flex items-center gap-2"><Zap className="size-4" />功耗与显示</CardTitle></CardHeader>
      <CardContent>
        <FieldGroup>
          <div className="grid gap-4 md:grid-cols-2">
            <Field><FieldLabel>功耗档位</FieldLabel><NativeSelect className="w-full" value={config.power.profile} onChange={(event) => setConfig({ ...config, power: { ...config.power, profile: event.target.value as 'mains' | 'battery' } })}><NativeSelectOption value="mains">常在线</NativeSelectOption><NativeSelectOption value="battery">电池</NativeSelectOption></NativeSelect></Field>
            <NumberField label="唤醒周期 / 秒" min={60} max={86400} value={config.power.wake_interval_sec} disabled={config.power.profile !== 'battery'} onChange={(wake_interval_sec) => setConfig({ ...config, power: { ...config.power, wake_interval_sec } })} />
          </div>
          <div className="grid gap-4 md:grid-cols-3">
            <NumberField label="局刷后全刷 / 次" min={1} max={100} value={config.display.full_after_partial_count} onChange={(full_after_partial_count) => setConfig({ ...config, display: { ...config.display, full_after_partial_count } })} />
            <NumberField label="全刷间隔 / 秒" min={3600} max={604800} value={config.display.full_max_age_sec} onChange={(full_max_age_sec) => setConfig({ ...config, display: { ...config.display, full_max_age_sec } })} />
            <NumberField label="面积阈值 / %" min={10} max={100} value={config.display.full_area_threshold_percent} onChange={(full_area_threshold_percent) => setConfig({ ...config, display: { ...config.display, full_area_threshold_percent } })} />
          </div>
        </FieldGroup>
      </CardContent>
    </Card>

    <Card>
      <CardHeader><CardTitle className="flex items-center gap-2"><Settings2 className="size-4" />设备</CardTitle></CardHeader>
      <CardContent><FieldGroup className="grid gap-4 md:grid-cols-3">
        <Field><FieldLabel>名称</FieldLabel><Input value={config.device.name} onChange={(event) => setConfig({ ...config, device: { ...config.device, name: event.target.value } })} /></Field>
        <Field><FieldLabel>Locale</FieldLabel><Input value={config.device.locale} onChange={(event) => setConfig({ ...config, device: { ...config.device, locale: event.target.value } })} /></Field>
        <Field><FieldLabel>时区</FieldLabel><Input value={config.device.timezone_iana} onChange={(event) => setConfig({ ...config, device: { ...config.device, timezone_iana: event.target.value } })} /></Field>
      </FieldGroup></CardContent>
    </Card>
  </>
)
