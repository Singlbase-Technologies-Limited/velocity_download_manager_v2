/** Mirrors `velocity_core::AppInfo` returned by the `app_info` Tauri command. */
export interface AppInfo {
  name: string;
  version: string;
}

/** Validates the native response before the UI uses it. */
export function parseAppInfo(value: unknown): AppInfo {
  if (typeof value === 'object' && value !== null) {
    const { name, version } = value as Record<string, unknown>;
    if (typeof name === 'string' && typeof version === 'string') {
      return { name, version };
    }
  }
  throw new Error('Unexpected app_info response');
}

export function windowHeading(info: AppInfo): string {
  return `${info.name} ${info.version}`;
}
