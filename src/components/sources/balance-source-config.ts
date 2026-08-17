import type { BalanceConfig, BalancePlatform } from '@/lib/agent'

const PLATFORM_TITLES: Record<BalancePlatform, string> = {
  deepseek: 'DeepSeek 余额',
  moonshot: 'Moonshot 余额',
}

export const newBalanceSource = (platform: BalancePlatform = 'deepseek'): BalanceConfig => ({
  id: '',
  enabled: true,
  title: PLATFORM_TITLES[platform],
  platform,
  interval_sec: 300,
  timeout_ms: 10_000,
  secret_configured: false,
})

export const balancePlatformTitle = (platform: BalancePlatform) => PLATFORM_TITLES[platform]
