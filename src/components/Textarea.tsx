import type { Ref, TextareaHTMLAttributes } from 'react';
import { cn } from '../lib/cn';
import { fieldClass } from './fieldStyles';

export function Textarea({ className, ref, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement> & { ref?: Ref<HTMLTextAreaElement> }) {
  return <textarea ref={ref} className={cn(fieldClass, 'resize-y py-1.5 font-mono text-xs', className)} spellCheck={false} {...props} />;
}
