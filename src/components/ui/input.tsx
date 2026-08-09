import type { InputHTMLAttributes } from 'react'
import { cn } from '../../lib/utils'

function Input({ className, type, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      type={type}
      className={cn(
        'h-10 w-full rounded-md border border-input bg-input/55 px-3 py-2 text-sm text-foreground shadow-[inset_0_1px_0_rgba(0,0,0,.05)] outline-none transition-colors placeholder:text-muted-foreground/65 focus:border-foreground focus:bg-card focus:ring-2 focus:ring-ring/25 disabled:cursor-not-allowed disabled:opacity-50',
        className,
      )}
      {...props}
    />
  )
}

export { Input }
