import type { InputHTMLAttributes, Ref } from 'react';
import { cn } from '../lib/cn';

const fieldClass =
  'w-full rounded-md border border-border bg-surface px-2.5 text-[13px] text-fg shadow-xs placeholder:text-fg-subtle focus-visible:border-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/25 disabled:opacity-50 aria-[invalid=true]:border-danger';

export function Input({ className, ref, ...props }: InputHTMLAttributes<HTMLInputElement> & { ref?: Ref<HTMLInputElement> }) {
  return <input ref={ref} className={cn(fieldClass, 'h-8', className)} spellCheck={false} {...props} />;
}
