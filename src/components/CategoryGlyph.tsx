import type { CategoryIcon } from '../contract';
import { CATEGORY_ICONS } from './categoryIcons';

/** A category's icon in its colour; decorative, as its name is beside it. */
export function CategoryGlyph({ icon, color, className }: { icon: CategoryIcon; color?: string; className?: string }) {
  const Icon = CATEGORY_ICONS[icon];
  return <Icon className={className ?? 'size-4'} style={color ? { color } : undefined} aria-hidden />;
}
