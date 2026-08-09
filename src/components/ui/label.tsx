import type { LabelHTMLAttributes } from 'react'
import { cn } from '../../lib/utils'

function Label({ className, ...props }: LabelHTMLAttributes<HTMLLabelElement>) {
  return <label className={cn('mb-1.5 block text-xs font-semibold tracking-wide text-muted-foreground', className)} {...props} />
}

export { Label }
