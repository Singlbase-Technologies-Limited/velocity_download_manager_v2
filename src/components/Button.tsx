import { cva, type VariantProps } from 'class-variance-authority';
import type { ButtonHTMLAttributes, Ref } from 'react';
import { cn } from '../lib/cn';

// Ported from V1 (src/components/ui/button.tsx) for #29.
const buttonVariants = cva(
  'inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-45 [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        primary: 'bg-accent text-accent-fg shadow-sm hover:bg-accent-strong',
        secondary: 'border border-border bg-surface text-fg shadow-sm hover:bg-surface-3',
        ghost: 'text-fg-muted hover:bg-surface-3 hover:text-fg',
        danger: 'bg-danger text-white shadow-sm hover:opacity-90',
      },
      size: {
        sm: 'h-7 px-2.5 text-xs',
        md: 'h-8 px-3 text-[13px]',
        lg: 'h-10 px-4 text-sm',
        icon: 'size-8',
        'icon-sm': 'size-7',
      },
    },
    defaultVariants: { variant: 'secondary', size: 'md' },
  },
);

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {
  /** An action this button started is running: show a spinner and refuse further clicks. */
  busy?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({ className, variant, size, type, busy = false, disabled, children, ref, ...props }: ButtonProps) {
  return (
    <button ref={ref} type={type ?? 'button'} className={cn(buttonVariants({ variant, size }), className)} disabled={busy || disabled} aria-busy={busy || undefined} {...props}>
      {busy ? <span aria-hidden className="inline-block size-3.5 animate-spin rounded-full border-2 border-current border-r-transparent" /> : null}
      {children}
    </button>
  );
}
