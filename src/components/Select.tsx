import type { Ref, SelectHTMLAttributes } from 'react';
import { cn } from '../lib/cn';
import { fieldClass } from './fieldStyles';

/** The system's own drop-down list, styled like the other fields: keyboard and screen readers work as everywhere. */
export function Select({ className, ref, ...props }: SelectHTMLAttributes<HTMLSelectElement> & { ref?: Ref<HTMLSelectElement> }) {
  return <select ref={ref} className={cn(fieldClass, 'h-8', className)} {...props} />;
}
