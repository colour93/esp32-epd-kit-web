import { cn } from '@/lib/utils';
const Progress = ({
  value = 0,
  className
}: {
  value?: number;
  className?: string;
}) => {
  const safeValue = Math.min(100, Math.max(0, value));
  return <div className={cn('h-1.5 w-full overflow-hidden rounded-full bg-primary/20', className)} role="progressbar" aria-valuenow={safeValue}>
      <div className="h-full bg-primary transition-[width] duration-500" style={{
      width: `${safeValue}%`
    }} />
    </div>;
};
export { Progress };
