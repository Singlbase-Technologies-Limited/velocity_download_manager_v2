import { useEffect } from 'react';
import type { Theme } from '../contract';

/** Applies the theme to <html>, following the system while it is `system` (V1's useTheme). */
export function useTheme(theme: Theme) {
  useEffect(() => {
    const root = document.documentElement;
    const query = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      root.classList.toggle('dark', theme === 'dark' || (theme === 'system' && query.matches));
    };
    apply();
    if (theme !== 'system') return;
    query.addEventListener('change', apply);
    return () => { query.removeEventListener('change', apply); };
  }, [theme]);
}
