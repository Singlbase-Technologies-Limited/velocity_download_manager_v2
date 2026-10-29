import { useId } from 'react';

/**
 * The Velocity app icon, drawn from `assets/brand/velocity-icon-small.svg` (the variant
 * simplified for small sizes). Decorative: the app name next to it is the accessible label.
 */
export function BrandIcon({ className }: { className?: string }) {
  const gradient = useId();
  return (
    <svg viewBox="0 0 1024 1024" className={className} aria-hidden focusable="false">
      <defs>
        <linearGradient id={gradient} x1="128" y1="0" x2="896" y2="1024" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#14B8C4" />
          <stop offset="0.48" stopColor="#1D6FD8" />
          <stop offset="1" stopColor="#2236B8" />
        </linearGradient>
      </defs>
      <rect x="16" y="16" width="992" height="992" rx="224" fill={`url(#${gradient})`} />
      <g fill="#FFFFFF">
        <path stroke="#FFFFFF" strokeWidth="24" strokeLinejoin="round" d="M216 232 H392 L512 486 L632 232 H808 L512 790 Z" />
        <rect x="300" y="842" width="424" height="86" rx="43" />
      </g>
    </svg>
  );
}
