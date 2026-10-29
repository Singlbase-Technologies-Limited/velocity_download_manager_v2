const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const;

/** Bytes in binary units with one decimal, as the tray says them (src-tauri/src/tray.rs): "1.5 MiB". */
export function formatBytes(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${String(bytes)} B` : `${value.toFixed(1)} ${UNITS[unit] ?? ''}`;
}

export function formatSpeed(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/s`;
}
