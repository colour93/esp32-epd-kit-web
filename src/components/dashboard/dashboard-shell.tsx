import type { ReactNode } from 'react'
import {
  Activity,
  Bot,
  Database,
  LayoutDashboard,
  Moon,
  Pause,
  Play,
  RefreshCw,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  type LucideIcon,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { StatusBadge } from './dashboard-components'

export type View = 'overview' | 'hardware' | 'resources' | 'sources' | 'security' | 'diagnostics'

const NAV_ITEMS: Array<{ id: View; label: string; icon: LucideIcon }> = [
  { id: 'overview', label: '概览', icon: LayoutDashboard },
  { id: 'hardware', label: '硬件', icon: SlidersHorizontal },
  { id: 'resources', label: '资源', icon: Database },
  { id: 'sources', label: '数据源', icon: Bot },
  { id: 'security', label: '主机', icon: ShieldCheck },
  { id: 'diagnostics', label: '诊断', icon: Activity },
]

type Theme = 'light' | 'dark'

const getInitialTheme = (): Theme => {
  const saved = window.localStorage.getItem('epd-theme')
  if (saved === 'light' || saved === 'dark') return saved
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

const ThemeButton = () => {
  const [theme, setTheme] = useState<Theme>(getInitialTheme)

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark')
    document.documentElement.style.colorScheme = theme
    window.localStorage.setItem('epd-theme', theme)
  }, [theme])

  return (
    <Tooltip>
      <TooltipTrigger render={<Button variant="ghost" size="icon" />} onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>
        {theme === 'dark' ? <Sun /> : <Moon />}
        <span className="sr-only">切换主题</span>
      </TooltipTrigger>
      <TooltipContent>切换主题</TooltipContent>
    </Tooltip>
  )
}

export const DashboardShell = ({
  children,
  view,
  onViewChange,
  devicePhase,
  agentVersion,
  platform,
  paused,
  busy,
  streamDown,
  connected,
  onReload,
  onPauseChange,
}: {
  children: ReactNode
  view: View
  onViewChange: (view: View) => void
  devicePhase: string
  agentVersion: string
  platform: string
  paused: boolean
  busy: boolean
  streamDown: boolean
  connected: boolean
  onReload: () => void
  onPauseChange: () => void
}) => {
  const current = NAV_ITEMS.find((item) => item.id === view) ?? NAV_ITEMS[0]

  return (
    <div className="min-h-screen bg-muted/30 text-foreground lg:grid lg:grid-cols-[240px_1fr]">
      <aside className="border-b bg-background lg:sticky lg:top-0 lg:h-screen lg:border-r lg:border-b-0">
        <div className="flex h-14 items-center gap-2 px-4 font-semibold">
          <span className="grid size-7 place-items-center rounded-md bg-primary font-mono text-xs text-primary-foreground">EP</span>
          <span>EPD Kit</span>
        </div>
        <Separator />
        <nav className="grid grid-cols-3 gap-1 p-2 sm:grid-cols-6 lg:grid-cols-1" aria-label="主导航">
          {NAV_ITEMS.map(({ id, label, icon: Icon }) => (
            <Button
              key={id}
              variant={view === id ? 'secondary' : 'ghost'}
              className="justify-start"
              onClick={() => onViewChange(id)}
            >
              <Icon data-icon="inline-start" />
              {label}
            </Button>
          ))}
        </nav>
        <div className="hidden lg:absolute lg:inset-x-0 lg:bottom-0 lg:block">
          <Separator />
          <div className="flex flex-col gap-3 p-4 text-xs text-muted-foreground">
            <div className="flex items-center justify-between gap-2"><span>设备</span><StatusBadge phase={devicePhase} /></div>
            <div className="font-mono">{agentVersion} · {platform}</div>
          </div>
        </div>
      </aside>

      <main className="min-w-0">
        <header className="sticky top-0 z-10 flex h-14 items-center justify-between border-b bg-background/95 px-4 backdrop-blur supports-backdrop-filter:bg-background/60 lg:px-6">
          <h1 className="text-sm font-semibold">{current.label}</h1>
          <div className="flex items-center gap-1">
            {streamDown ? <StatusBadge phase="reconnecting" /> : null}
            <ThemeButton />
            <Tooltip>
              <TooltipTrigger render={<Button variant="ghost" size="icon" disabled={!connected || busy} />} onClick={onReload}>
                <RefreshCw />
                <span className="sr-only">刷新</span>
              </TooltipTrigger>
              <TooltipContent>刷新</TooltipContent>
            </Tooltip>
            <Button variant="outline" onClick={onPauseChange} disabled={busy}>
              {paused ? <Play data-icon="inline-start" /> : <Pause data-icon="inline-start" />}
              {paused ? '恢复' : '暂停'}
            </Button>
          </div>
        </header>
        <div className={cn('mx-auto flex w-full max-w-7xl flex-col gap-4 p-4 lg:p-6')}>{children}</div>
      </main>
    </div>
  )
}
