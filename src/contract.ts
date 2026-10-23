// The UI side of the native contract (#25, docs/CONTRACTS.md). Every response from the native
// side is validated with these schemas before use. They mirror crates/velocity-core/src/domain.rs
// and are checked against fixtures generated from it (contract.test.ts), so any drift fails a test.
// Objects are strict: a field the UI doesn't know is an error, not something to ignore.
import { z } from 'zod';

/** Must equal `velocity_core::COMMAND_API_VERSION`; the app refuses to start on a mismatch. */
export const COMMAND_API_VERSION = 3;

const bytes = z.number().int().nonnegative();

export const DownloadStatusSchema = z.enum([
  'queued',
  'connecting',
  'downloading',
  'paused',
  'verifying',
  'retrying',
  'stalled',
  'completed',
  'failed',
  'removed',
  'fetchingMetadata',
  'choosingFiles',
  'seeding',
  'unknown',
]);
export type DownloadStatus = z.infer<typeof DownloadStatusSchema>;

export const DownloadKindSchema = z.enum(['http', 'ftp', 'sftp', 'metalink', 'torrent', 'magnet']);
export type DownloadKind = z.infer<typeof DownloadKindSchema>;

export const FailureClassSchema = z.enum(['transient', 'throttled', 'permanent']);

export const TransferErrorSchema = z.strictObject({
  class: FailureClassSchema,
  httpStatus: z.number().int().min(100).max(599).nullable(),
  message: z.string(),
  retryable: z.boolean(),
});
export type TransferError = z.infer<typeof TransferErrorSchema>;

export const ConnectionLimitSchema = z.enum([
  'perHostCap',
  'sharedBudget',
  'serverThrottling',
  'rampingUp',
  'noGain',
  'fileSize',
]);
export type ConnectionLimit = z.infer<typeof ConnectionLimitSchema>;

export const ConnectionDetailSchema = z.strictObject({
  requested: z.number().int().positive(),
  allowed: z.number().int().positive(),
  limitedBy: ConnectionLimitSchema.nullable(),
});
export type ConnectionDetail = z.infer<typeof ConnectionDetailSchema>;

/** What was proven about a completed download's bytes (#46, ADR-0010). Only `verified` means a hash was checked. */
export const VerificationSchema = z.enum(['verified', 'sizeVerified', 'notVerified']);
export type Verification = z.infer<typeof VerificationSchema>;

export const ProgressSchema = z.strictObject({
  completedBytes: bytes,
  totalBytes: bytes.nullable(),
  downloadSpeedBps: bytes,
  uploadSpeedBps: bytes,
  connections: z.number().int().nonnegative(),
  connectionDetail: ConnectionDetailSchema.nullable(),
});
export type Progress = z.infer<typeof ProgressSchema>;

export const DownloadRecordSchema = z.strictObject({
  id: z.string().min(1),
  kind: DownloadKindSchema,
  sourceUrl: z.string(),
  fileName: z.string().nullable(),
  directory: z.string(),
  categoryId: z.string().nullable(),
  status: DownloadStatusSchema,
  progress: ProgressSchema,
  error: TransferErrorSchema.nullable(),
  verification: VerificationSchema.nullable(),
  createdAtMs: bytes,
  completedAtMs: bytes.nullable(),
});
export type DownloadRecord = z.infer<typeof DownloadRecordSchema>;

export const CapabilitiesSchema = z.strictObject({
  protocols: z.array(DownloadKindSchema),
  liveConnectionChanges: z.boolean(),
  validatorResume: z.boolean(),
  metalink: z.boolean(),
  maxConnectionsPerHost: z.number().int().positive(),
});
export type Capabilities = z.infer<typeof CapabilitiesSchema>;

export const ErrorCodeSchema = z.enum([
  'INVALID_INPUT',
  'INVALID_URL',
  'UNSUPPORTED_SCHEME',
  'INVALID_PATH',
  'INVALID_FILENAME',
  'DESTINATION_MISSING',
  'PERMISSION_DENIED',
  'FILE_EXISTS',
  'NOT_FOUND',
  'INVALID_STATE',
  'ENGINE_MISSING',
  'ENGINE_INCOMPATIBLE',
  'ENGINE_START_FAILED',
  'ENGINE_UNAVAILABLE',
  'DATABASE',
  'DATABASE_MIGRATION',
  'IO',
  'INSUFFICIENT_DISK_SPACE',
  'PROTOCOL_DISABLED',
  'CONFIRMATION_REQUIRED',
  'UNSUPPORTED',
  'INTERNAL',
]);
export type ErrorCode = z.infer<typeof ErrorCodeSchema>;

export const AppInfoSchema = z.strictObject({
  name: z.string().min(1),
  version: z.string().min(1),
  commandApiVersion: z.number().int().positive(),
});
export type AppInfo = z.infer<typeof AppInfoSchema>;

/** Validates the `app_info` response and refuses a native side built for another contract. */
export function parseAppInfo(value: unknown): AppInfo {
  const parsed = AppInfoSchema.safeParse(value);
  if (!parsed.success) throw new Error('Unexpected app_info response');
  if (parsed.data.commandApiVersion !== COMMAND_API_VERSION) {
    throw new Error(
      `The native side uses contract version ${String(parsed.data.commandApiVersion)}, but this window expects ${String(COMMAND_API_VERSION)}. Reinstall or rebuild Velocity so both parts match.`,
    );
  }
  return parsed.data;
}
