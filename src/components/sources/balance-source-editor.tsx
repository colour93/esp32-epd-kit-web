import { CircleDollarSign, Trash2, Undo2 } from 'lucide-react'
import type { BalanceConfig, BalancePlatform } from '@/lib/agent'
import { balancePlatformTitle } from '@/components/sources/balance-source-config'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldTitle } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'

const PLATFORM_DETAILS: Record<BalancePlatform, { endpoint: string; metrics: string }> = {
  deepseek: {
    endpoint: 'api.deepseek.com',
    metrics: '总余额 / 充值余额 / 赠送余额',
  },
  moonshot: {
    endpoint: 'api.moonshot.cn',
    metrics: '可用余额 / 现金余额 / 赠送余额',
  },
}

export const BalanceSourceEditor = ({
  value,
  creating,
  onChange,
}: {
  value: BalanceConfig
  creating: boolean
  onChange: (value: BalanceConfig) => void
}) => {
  const details = PLATFORM_DETAILS[value.platform]
  const configured = Boolean(value.secret_configured)
  const changePlatform = (platform: BalancePlatform) => onChange({
    ...value,
    platform,
    title: balancePlatformTitle(platform),
    api_key: undefined,
    clear_secret: configured,
  })

  return (
    <FieldGroup className="gap-6">
      <Field>
        <FieldLabel>平台</FieldLabel>
        <ToggleGroup className="grid w-full grid-cols-2" value={[value.platform]} onValueChange={(selected: string[]) => selected[0] && changePlatform(selected[0] as BalancePlatform)} variant="outline" spacing={0}>
          <ToggleGroupItem value="deepseek">DeepSeek</ToggleGroupItem>
          <ToggleGroupItem value="moonshot">Moonshot</ToggleGroupItem>
        </ToggleGroup>
      </Field>

      <Alert>
        <CircleDollarSign />
        <AlertTitle>{balancePlatformTitle(value.platform)}</AlertTitle>
        <AlertDescription>
          <div className="font-mono text-xs">{details.endpoint}</div>
          <div className="mt-2">{details.metrics}</div>
        </AlertDescription>
      </Alert>

      <Field orientation="horizontal">
        <FieldTitle>启用</FieldTitle>
        <Switch checked={value.enabled} onCheckedChange={(enabled: boolean) => onChange({ ...value, enabled })} />
      </Field>

      <FieldGroup className="grid gap-4 md:grid-cols-2">
        <Field>
          <FieldLabel>实例 ID</FieldLabel>
          <Input maxLength={32} disabled={!creating} value={value.id} onChange={(event) => onChange({ ...value, id: event.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, '') })} />
        </Field>
        <Field>
          <FieldLabel>名称</FieldLabel>
          <Input maxLength={32} value={value.title} onChange={(event) => onChange({ ...value, title: event.target.value })} />
        </Field>
        <Field>
          <FieldLabel>轮询间隔</FieldLabel>
          <NativeSelect className="w-full" value={value.interval_sec} onChange={(event) => onChange({ ...value, interval_sec: Number(event.target.value) })}>
            <NativeSelectOption value={60}>1 分钟</NativeSelectOption>
            <NativeSelectOption value={300}>5 分钟</NativeSelectOption>
            <NativeSelectOption value={900}>15 分钟</NativeSelectOption>
            <NativeSelectOption value={1800}>30 分钟</NativeSelectOption>
            <NativeSelectOption value={3600}>1 小时</NativeSelectOption>
          </NativeSelect>
        </Field>
        <Field>
          <FieldLabel>请求超时</FieldLabel>
          <NativeSelect className="w-full" value={value.timeout_ms} onChange={(event) => onChange({ ...value, timeout_ms: Number(event.target.value) })}>
            <NativeSelectOption value={3000}>3 秒</NativeSelectOption>
            <NativeSelectOption value={5000}>5 秒</NativeSelectOption>
            <NativeSelectOption value={10000}>10 秒</NativeSelectOption>
            <NativeSelectOption value={20000}>20 秒</NativeSelectOption>
            <NativeSelectOption value={30000}>30 秒</NativeSelectOption>
          </NativeSelect>
        </Field>
      </FieldGroup>

      <Field>
        <FieldLabel>API Key</FieldLabel>
        <div className="flex min-w-0 flex-col gap-2 sm:flex-row">
          <Input
            className="min-w-0 font-mono"
            type="password"
            autoComplete="new-password"
            placeholder={configured && !value.clear_secret ? '已配置；留空保持不变' : '输入 API Key'}
            value={value.api_key ?? ''}
            onChange={(event) => onChange({ ...value, api_key: event.target.value || undefined, clear_secret: false })}
          />
          {configured ? (
            value.clear_secret ? (
              <Button variant="outline" type="button" onClick={() => onChange({ ...value, clear_secret: false })}>
                <Undo2 data-icon="inline-start" />
                保留密钥
              </Button>
            ) : (
              <Button variant="outline" type="button" onClick={() => onChange({ ...value, api_key: undefined, clear_secret: true })}>
                <Trash2 data-icon="inline-start" />
                清除密钥
              </Button>
            )
          ) : null}
        </div>
        <FieldDescription>{value.clear_secret ? '保存后清除已配置的密钥。' : configured ? '输入新值会替换已配置的密钥。' : 'API Key 保存在系统凭据库中。'}</FieldDescription>
      </Field>
    </FieldGroup>
  )
}
