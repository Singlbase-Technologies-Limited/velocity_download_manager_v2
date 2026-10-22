import type { AppInfo } from './contract';

/** The window heading: product name and version. Validation lives in `contract.ts`. */
export function windowHeading(info: AppInfo): string {
  return `${info.name} ${info.version}`;
}
