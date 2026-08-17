import { Plus, ShieldAlert, Trash2, Undo2 } from 'lucide-react'
import type {
  HttpMetricAuthConfig,
  HttpMetricConfig,
} from '@/lib/agent'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldTitle } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { MetricItemsEditor } from '@/components/sources/metric-items-editor'

const updateAuthType = (current: HttpMetricAuthConfig, type: HttpMetricAuthConfig['type']): HttpMetricAuthConfig => ({
  type,
  header_name: type === 'header' ? (current.header_name || 'X-API-Key') : undefined,
  secret: type === 'none' ? undefined : current.secret,
  secret_configured: current.secret_configured,
  clear_secret: type === 'none' ? Boolean(current.secret_configured) : false,
})

const SecretField = ({ auth, label, onChange }: {
  auth: HttpMetricAuthConfig
  label?: string
  onChange: (auth: HttpMetricAuthConfig) => void
}) => {
  if (auth.type === 'none') return null
  const configured = Boolean(auth.secret_configured)
  return (
    <Field>
      <FieldLabel>{label ?? (auth.type === 'bearer' ? 'Bearer Token' : 'Header 密钥')}</FieldLabel>
      <div className="flex min-w-0 flex-col gap-2 sm:flex-row">
        <Input
          className="min-w-0 font-mono"
          type="password"
          autoComplete="new-password"
          placeholder={configured && !auth.clear_secret ? '已配置；留空保持不变' : '输入密钥'}
          value={auth.secret ?? ''}
          onChange={(event) => onChange({ ...auth, secret: event.target.value || undefined, clear_secret: false })}
        />
        {configured ? (
          auth.clear_secret ? (
            <Button variant="outline" type="button" onClick={() => onChange({ ...auth, clear_secret: false })}>
              <Undo2 data-icon="inline-start" />
              保留密钥
            </Button>
          ) : (
            <Button variant="outline" type="button" onClick={() => onChange({ ...auth, secret: undefined, clear_secret: true })}>
              <Trash2 data-icon="inline-start" />
              清除密钥
            </Button>
          )
        ) : null}
      </div>
      <FieldDescription>{auth.clear_secret ? '保存后清除已配置的密钥。' : configured ? '输入新值会替换已配置的密钥。' : '密钥不会显示在数据源列表中。'}</FieldDescription>
    </Field>
  )
}

export const HttpSourceEditor = ({
  value,
  creating,
  onChange,
}: {
  value: HttpMetricConfig
  creating: boolean
  onChange: (value: HttpMetricConfig) => void
}) => {
  return (
    <FieldGroup className="gap-6">
      <Field orientation="horizontal">
        <FieldTitle>启用</FieldTitle>
        <Switch checked={value.enabled} onCheckedChange={(enabled) => onChange({ ...value, enabled })} />
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

      <FieldGroup className="grid gap-4 lg:grid-cols-2">
            <Field>
              <FieldLabel>请求方法</FieldLabel>
              <ToggleGroup className="w-full" value={[value.method]} onValueChange={(selected) => selected[0] && onChange({ ...value, method: selected[0] as HttpMetricConfig['method'], body: selected[0] === 'GET' ? '' : value.body })} variant="outline" spacing={0}>
                <ToggleGroupItem className="flex-1" value="GET">GET</ToggleGroupItem>
                <ToggleGroupItem className="flex-1" value="POST">POST</ToggleGroupItem>
              </ToggleGroup>
            </Field>
            <Field>
              <FieldLabel>网络范围</FieldLabel>
              <ToggleGroup className="grid w-full grid-cols-3" value={[value.network_access]} onValueChange={(selected) => selected[0] && onChange({ ...value, network_access: selected[0] as HttpMetricConfig['network_access'] })} variant="outline" spacing={0}>
                <ToggleGroupItem value="public">公网</ToggleGroupItem>
                <ToggleGroupItem value="private">内网</ToggleGroupItem>
                <ToggleGroupItem value="localhost">本机</ToggleGroupItem>
              </ToggleGroup>
            </Field>
            <Field className="lg:col-span-2">
              <FieldLabel>URL</FieldLabel>
              <Input className="font-mono" type="url" placeholder="https://api.example.com/usage" value={value.url} onChange={(event) => onChange({ ...value, url: event.target.value })} />
              <FieldDescription>使用 {'{{today}}'} 插入本机当日日期（YYYY-MM-DD）。</FieldDescription>
            </Field>
      </FieldGroup>

      {value.network_access === 'public' ? null : (
        <Alert>
          <ShieldAlert />
          <AlertTitle>{value.network_access === 'localhost' ? '允许访问本机服务' : '允许访问内网服务'}</AlertTitle>
          <AlertDescription>仅为可信目标启用此范围。</AlertDescription>
        </Alert>
      )}

      <FieldGroup className="grid gap-4 lg:grid-cols-2">
            <Field>
              <FieldLabel>认证方式</FieldLabel>
              <ToggleGroup className="grid w-full grid-cols-3" value={[value.auth.type]} onValueChange={(selected) => selected[0] && onChange({ ...value, auth: updateAuthType(value.auth, selected[0] as HttpMetricAuthConfig['type']) })} variant="outline" spacing={0}>
                <ToggleGroupItem value="none">无</ToggleGroupItem>
                <ToggleGroupItem value="bearer">Bearer</ToggleGroupItem>
                <ToggleGroupItem value="header">Header</ToggleGroupItem>
              </ToggleGroup>
            </Field>
            {value.auth.type === 'header' ? (
              <Field>
                <FieldLabel>密钥 Header</FieldLabel>
                <Input className="font-mono" placeholder="X-API-Key" value={value.auth.header_name ?? ''} onChange={(event) => onChange({ ...value, auth: { ...value.auth, header_name: event.target.value } })} />
              </Field>
            ) : null}
            <SecretField auth={value.auth} onChange={(auth) => onChange({ ...value, auth })} />
      </FieldGroup>

      <FieldGroup className="gap-3">
            <FieldLabel>请求 Header</FieldLabel>
            {value.headers.map((header, index) => (
              <FieldGroup key={index} className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
                <Field><FieldLabel className="sr-only">Header 名称</FieldLabel><Input className="font-mono" placeholder="Header" value={header.name} onChange={(event) => onChange({ ...value, headers: value.headers.map((item, itemIndex) => itemIndex === index ? { ...item, name: event.target.value } : item) })} /></Field>
                <Field><FieldLabel className="sr-only">Header 值</FieldLabel><Input className="font-mono" placeholder="Value" value={header.value} onChange={(event) => onChange({ ...value, headers: value.headers.map((item, itemIndex) => itemIndex === index ? { ...item, value: event.target.value } : item) })} /></Field>
                <Button variant="ghost" size="icon" onClick={() => onChange({ ...value, headers: value.headers.filter((_, itemIndex) => itemIndex !== index) })}><Trash2 /><span className="sr-only">删除 Header</span></Button>
              </FieldGroup>
            ))}
            <Button variant="outline" size="sm" className="self-start" onClick={() => onChange({ ...value, headers: [...value.headers, { name: '', value: '' }] })}>
              <Plus data-icon="inline-start" />
              添加 Header
            </Button>
      </FieldGroup>

      {value.method === 'POST' ? (
        <Field>
          <FieldLabel>JSON Body</FieldLabel>
          <Textarea className="min-h-32 font-mono" spellCheck={false} value={value.body} onChange={(event) => onChange({ ...value, body: event.target.value })} />
        </Field>
      ) : null}

      <MetricItemsEditor items={value.items} onChange={(items) => onChange({ ...value, items })} />
    </FieldGroup>
  )
}
