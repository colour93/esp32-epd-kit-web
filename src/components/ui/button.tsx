import type { ButtonHTMLAttributes } from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '../../lib/utils'

const buttonVariants = cva(
  'inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 rounded-md border text-sm font-semibold transition-[transform,background,color,border-color,box-shadow] outline-none select-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-45 active:translate-y-px [&_svg]:pointer-events-none [&_svg]:size-4',
  {
    variants: {
      variant: {
        default: 'border-primary bg-primary text-primary-foreground shadow-[0_2px_0_#000] hover:bg-primary/90',
        signal: 'border-signal bg-signal text-signal-foreground shadow-[0_2px_0_#273100] hover:bg-signal/85',
        outline: 'border-border bg-card text-foreground shadow-[0_1px_0_rgba(0,0,0,.18)] hover:bg-muted',
        ghost: 'border-transparent bg-transparent text-muted-foreground hover:bg-muted hover:text-foreground',
        destructive: 'border-destructive bg-destructive text-white shadow-[0_2px_0_#6e1717] hover:bg-destructive/90',
      },
      size: {
        default: 'h-10 px-4',
        sm: 'h-8 rounded-sm px-3 text-xs',
        lg: 'h-12 px-5 text-[15px]',
        icon: 'size-10 p-0',
      },
    },
    defaultVariants: { variant: 'default', size: 'default' },
  },
)

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & VariantProps<typeof buttonVariants>

function Button({ className, variant, size, type = 'button', ...props }: ButtonProps) {
  return <button type={type} className={cn(buttonVariants({ variant, size }), className)} {...props} />
}

export { Button }
