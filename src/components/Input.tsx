import type { InputHTMLAttributes, Ref } from 'react';
import { cn } from '../lib/cn';
import { fieldClass } from './fieldStyles';

export function Input({ className, ref, ...props }: InputHTMLAttributes<HTMLInputElement> & { ref?: Ref<HTMLInputElement> }) {
  return <input ref={ref} className={cn(fieldClass, 'h-8', className)} spellCheck={false} {...props} />;
}
