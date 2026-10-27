// The UI side of the native contract (#25, docs/CONTRACTS.md). Every response from the native
// side is validated with these schemas before use. They mirror crates/velocity-core/src/domain.rs
// and are checked against fixtures generated from it (contract.test.ts), so any drift fails a test.
// Objects are strict: a field the UI doesn't know is an error, not something to ignore.
import { z } from 'zod';

/** Must equal `velocity_core::COMMAND_API_VERSION`; the app refuses to start on a mismatch. */
export const COMMAND_API_VERSION = 8;

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

/** An SFTP server host key the user hasn’t trusted (#53): unknown until they confirm its fingerprint, or changed, which is always refused. */
export const HostKeyCheckSchema = z.strictObject({
  host: z.string(),
  port: z.number().int().min(1).max(65535),
  fingerprint: z.string(),
  pinned: z.string().nullable(),
});
export type HostKeyCheck = z.infer<typeof HostKeyCheckSchema>;

export const TransferErrorSchema = z.strictObject({
  class: FailureClassSchema,
  httpStatus: z.number().int().min(100).max(599).nullable(),
  message: z.string(),
  retryable: z.boolean(),
  hostKey: HostKeyCheckSchema.nullable(),
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

/** What an engine does for one protocol (#57). A protocol it doesn't list isn't supported. */
export const ProtocolCapabilitiesSchema = z.strictObject({
  kind: DownloadKindSchema,
  segmented: z.boolean(),
  resume: z.boolean(),
  validatorResume: z.boolean(),
  http2: z.boolean(),
  /** The connection can be encrypted, with the certificate checked: https://, FTPS (#51). */
  encryption: z.boolean(),
});
export type ProtocolCapabilities = z.infer<typeof ProtocolCapabilitiesSchema>;

export const CapabilitiesSchema = z.strictObject({
  protocols: z.array(ProtocolCapabilitiesSchema),
  liveConnectionChanges: z.boolean(),
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
  'NAME_TAKEN',
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

/** Whether the transfer engine can take commands (#39, #58). */
export const EngineHealthSchema = z.discriminatedUnion('state', [
  z.strictObject({ state: z.literal('running') }),
  /** It stopped and is being started again; `attempt` counts the crashes in the restart window. */
  z.strictObject({ state: z.literal('restarting'), attempt: z.number().int().nonnegative() }),
  /** It won't start again on its own: "Restart transfer engine" tries again. */
  z.strictObject({ state: z.literal('unavailable'), code: ErrorCodeSchema }),
]);
export type EngineHealth = z.infer<typeof EngineHealthSchema>;

/** A stored download this build can't act on: shown, never acted on. */
export const UnavailableDownloadSchema = z.strictObject({
  id: z.string().min(1),
  sourceUrl: z.string(),
  fileName: z.string().nullable(),
  directory: z.string(),
  createdAtMs: bytes,
});
export type UnavailableDownload = z.infer<typeof UnavailableDownloadSchema>;

/** What the UI is sent about the downloads (#58): the list, in order, and the state around it. */
export const DownloadsSnapshotSchema = z.strictObject({
  downloads: z.array(DownloadRecordSchema),
  unavailable: z.array(UnavailableDownloadSchema),
  /** Downloads that came back after a restart without their login: resuming one asks for it. */
  needsLogin: z.array(z.string().min(1)),
  engine: EngineHealthSchema,
  /** The torrent helper's health (#58). */
  torrentEngine: EngineHealthSchema,
  storageError: ErrorCodeSchema.nullable(),
});
export type DownloadsSnapshot = z.infer<typeof DownloadsSnapshotSchema>;

/** Why a removal with files left a file in place (#58). */
export const KeptReasonSchema = z.enum(['notPlain', 'tooLarge', 'behindLink', 'notEmpty', 'failed']);
export type KeptReason = z.infer<typeof KeptReasonSchema>;

/** What a command over several downloads did (#58). */
export const BatchResultSchema = z.strictObject({
  done: z.array(z.string().min(1)),
  failed: z.array(z.strictObject({ id: z.string().min(1), code: ErrorCodeSchema })),
  kept: z.array(z.strictObject({ path: z.string(), reason: KeptReasonSchema, code: ErrorCodeSchema.nullable() })),
});
export type BatchResult = z.infer<typeof BatchResultSchema>;

/** The icons a category may have (#31), in the order the UI offers them. */
export const CategoryIconSchema = z.enum([
  'folder',
  'archive',
  'file-text',
  'music',
  'film',
  'app-window',
  'disc',
  'image',
  'code',
  'book',
  'package',
  'star',
]);
export type CategoryIcon = z.infer<typeof CategoryIconSchema>;

/** A download category (#31): built in (editable, not deletable) or the user's. */
export const CategorySchema = z.strictObject({
  id: z.string().min(1),
  name: z.string().min(1).max(40),
  color: z.string().regex(/^#[0-9a-f]{6}$/),
  icon: CategoryIconSchema,
  /** Lowercase, without the dot. */
  extensions: z.array(z.string().regex(/^[0-9a-z]{1,16}$/)).max(100),
  /** The category's own folder, if it has one. */
  directory: z.string().min(1).nullable(),
  builtin: z.boolean(),
});
export type Category = z.infer<typeof CategorySchema>;

/** What adding a download would do, before it is added (#31). Free disk space isn't reported yet. */
export const DestinationCheckSchema = z.strictObject({
  directory: z.string().min(1),
  directoryExists: z.boolean(),
  /** The folder came from the category or the settings, not from the user. */
  automaticDirectory: z.boolean(),
  categoryId: z.string().min(1).nullable(),
  /** `null` when the server will name the file. */
  fileName: z.string().min(1).nullable(),
  fileExists: z.boolean(),
  /** A download, V2's or V1's, that hasn't finished has the name. */
  partialDownloadExists: z.boolean(),
  /** When the name is taken, the first free `name (n).ext`. */
  suggestedName: z.string().min(1).nullable(),
});
export type DestinationCheck = z.infer<typeof DestinationCheckSchema>;

/** What adding does when the name is taken (#31): ask, or take the first free name. Never replace. */
export const OnConflictSchema = z.enum(['ask', 'rename']);
export type OnConflict = z.infer<typeof OnConflictSchema>;

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
