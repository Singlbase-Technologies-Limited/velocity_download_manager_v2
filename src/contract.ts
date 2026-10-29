// The UI side of the native contract (#25, docs/CONTRACTS.md). Every response from the native
// side is validated with these schemas before use. They mirror crates/velocity-core/src/domain.rs
// and are checked against fixtures generated from it (contract.test.ts), so any drift fails a test.
// Objects are strict: a field the UI doesn't know is an error, not something to ignore.
import { z } from 'zod';

/** Must equal `velocity_core::COMMAND_API_VERSION`; the app refuses to start on a mismatch. */
export const COMMAND_API_VERSION = 14;

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

/** A torrent's peers and seeding, as the torrent helper reports them (#61). */
export const TorrentActivitySchema = z.strictObject({
  peers: z.number().int().nonnegative(),
  seeds: z.number().int().nonnegative(),
  uploadedBytes: bytes,
  seedingSecs: bytes,
});
export type TorrentActivity = z.infer<typeof TorrentActivitySchema>;

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
  /** While retrying, when the engine tries again (#59); `null` in every other status. */
  retryAtMs: bytes.nullable(),
  /** The first time the engine reported it running (#59). */
  startedAtMs: bytes.nullable(),
  /** Bytes per second while it ran, once it ran for a second (#59). */
  averageSpeedBps: bytes.nullable(),
  /** Its own speed limit, in bytes per second; `null` for none (#61). */
  speedLimitBps: bytes.nullable(),
  /** The connections a direct download asks for; `null` for a torrent (#61). */
  requestedConnections: z.number().int().positive().nullable(),
  /** What the torrent helper last reported about a torrent's peers and seeding; `null` for a direct download, and until it reports (#61). */
  torrent: TorrentActivitySchema.nullable(),
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

/** A custom request header (#40). Its value may be a secret: kept in memory only. */
export const HeaderSchema = z.strictObject({ name: z.string(), value: z.string() });
export type Header = z.infer<typeof HeaderSchema>;

/** A login for a download's server (#40), in memory only. */
export const CredentialsSchema = z.strictObject({ userName: z.string(), password: z.string() });
export type Credentials = z.infer<typeof CredentialsSchema>;

/** A private key file for an SFTP login (#53), and its passphrase. */
export const SshKeySchema = z.strictObject({ path: z.string().min(1), passphrase: z.string().nullable() });
export type SshKey = z.infer<typeof SshKeySchema>;

export const HashAlgorithmSchema = z.enum(['md5', 'sha1', 'sha224', 'sha256', 'sha384', 'sha512']);
export type HashAlgorithm = z.infer<typeof HashAlgorithmSchema>;

/** What a download that came back after a restart without them is given again (#59): `set_download_login`. */
export const DownloadLoginSchema = z.strictObject({
  credentials: CredentialsSchema.nullable(),
  sshKey: SshKeySchema.nullable(),
  headers: z.array(HeaderSchema),
});
export type DownloadLogin = z.infer<typeof DownloadLoginSchema>;

/** Where `move_download` takes a waiting download in the queue (#50). */
export type MoveDirection = 'top' | 'up' | 'down' | 'bottom';

/** A whole-file hash to verify before the download completes (#46). */
export const ChecksumSchema = z.strictObject({ algorithm: HashAlgorithmSchema, hex: z.string() });
export type Checksum = z.infer<typeof ChecksumSchema>;

/** What the Add dialog asks for (#60): the links and `.torrent` files, and the choices for each. */
export const AddRequestSchema = z.strictObject({
  links: z.array(z.string()),
  torrentFiles: z.array(z.string()),
  directory: z.string().nullable(),
  /** For a single direct link only. */
  fileName: z.string().nullable(),
  categoryId: z.string().nullable(),
  /** Create the chosen folder when it is missing: the user agreed to. */
  createDirectory: z.boolean(),
  onConflict: OnConflictSchema,
  /** "Add anyway": add what was added before, except a torrent still in the list. */
  allowDuplicate: z.boolean(),
  startPaused: z.boolean(),
  /** `null` for the performance profile's. */
  connections: z.number().int().positive().nullable(),
  /** KiB/s; 0 for no limit. */
  speedLimitKib: z.number().int().nonnegative(),
  headers: z.array(HeaderSchema),
  referrer: z.string().nullable(),
  credentials: CredentialsSchema.nullable(),
  sshKey: SshKeySchema.nullable(),
  checksum: ChecksumSchema.nullable(),
  requireEncryption: z.boolean(),
  chooseFiles: z.boolean(),
});
export type AddRequest = z.infer<typeof AddRequestSchema>;

/** The download a new one would repeat (#60). */
export const DuplicateSchema = z.strictObject({
  existingId: z.string().min(1),
  name: z.string().nullable(),
  status: DownloadStatusSchema,
  inList: z.boolean(),
  /** False only for a torrent still in the list: two copies can't run. */
  canAddAnyway: z.boolean(),
});
export type Duplicate = z.infer<typeof DuplicateSchema>;

/** What adding one link or `.torrent` file did (#60). */
export const AddOutcomeSchema = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('added'), id: z.string().min(1) }),
  /** The name is taken: nothing was added. */
  z.strictObject({ kind: z.literal('conflict'), check: DestinationCheckSchema }),
  /** It was added before: nothing was added. */
  z.strictObject({ kind: z.literal('duplicate'), duplicate: DuplicateSchema }),
  z.strictObject({ kind: z.literal('failed'), code: ErrorCodeSchema }),
  /** A `.torrent` link couldn't be fetched: the server's or the network's reason. */
  z.strictObject({ kind: z.literal('notFetched'), error: TransferErrorSchema }),
]);
export type AddOutcome = z.infer<typeof AddOutcomeSchema>;

export const AddResultSchema = z.strictObject({ source: z.string(), outcome: AddOutcomeSchema });
export type AddResult = z.infer<typeof AddResultSchema>;

/** The `torrents-dropped` event (#60): `.torrent` files dropped on the window, and how many other things were. */
export const DroppedTorrentsSchema = z.strictObject({
  paths: z.array(z.string().min(1)),
  ignored: z.number().int().nonnegative(),
});
export type DroppedTorrents = z.infer<typeof DroppedTorrentsSchema>;

/** The colour theme (#29): the system's, or always light or dark. */
export const ThemeSchema = z.enum(['system', 'light', 'dark']);
export type Theme = z.infer<typeof ThemeSchema>;

/** How tightly lists are laid out (#29). */
export const DensitySchema = z.enum(['compact', 'comfortable']);
export type Density = z.infer<typeof DensitySchema>;

/** Panel width bounds in CSS pixels, as Rust checks them (`app_settings.rs`, src/layout/layoutLogic.ts). */
const width = (min: number, max: number) => z.number().int().min(min).max(max).nullable();

/** The window's panels (#29). A `null` width is the responsive default. */
export const LayoutSettingsSchema = z.strictObject({
  sidebarWidth: width(184, 360),
  sidebarCollapsed: z.boolean(),
  sidebarVisible: z.boolean(),
  detailsWidth: width(280, 600),
  detailsVisible: z.boolean(),
});
export type LayoutSettings = z.infer<typeof LayoutSettingsSchema>;

/** A column of the download list (#59, V1's ids), in the order the column chooser lists them. */
export const ColumnIdSchema = z.enum([
  'name',
  'size',
  'downloaded',
  'progress',
  'speed',
  'avgSpeed',
  'eta',
  'status',
  'category',
  'destination',
  'added',
  'started',
  'completed',
]);
export type ColumnId = z.infer<typeof ColumnIdSchema>;

/** The columns of a new install (V1's), as `ColumnId::DEFAULT` in `app_settings.rs`. */
export const DEFAULT_COLUMNS: readonly ColumnId[] = ['name', 'size', 'progress', 'speed', 'eta', 'status', 'category', 'added'];

/** Bounds for a column width the user set, in CSS pixels, as Rust checks them. */
export const COLUMN_MIN_WIDTH = 48;
export const COLUMN_MAX_WIDTH = 1200;

/** How the window looks (#29), and the download list's columns (#59). */
export const AppearanceSettingsSchema = z.strictObject({
  theme: ThemeSchema,
  density: DensitySchema,
  layout: LayoutSettingsSchema,
  /** In display order; always with `name`, and each column once. */
  visibleColumns: z.array(ColumnIdSchema).min(1),
  /** Only the columns the user resized; the rest have their default width. */
  columnWidths: z.partialRecord(ColumnIdSchema, z.number().int().min(COLUMN_MIN_WIDTH).max(COLUMN_MAX_WIDTH)),
});
export type AppearanceSettings = z.infer<typeof AppearanceSettingsSchema>;

/** What the window starts with (#29), from `get_shell_settings`. */
export const ShellSettingsSchema = z.strictObject({
  appearance: AppearanceSettingsSchema,
  onboardingCompleted: z.boolean(),
  /** The folder downloads go to now; `null` when the system names none. */
  defaultDirectory: z.string().min(1).nullable(),
});
export type ShellSettings = z.infer<typeof ShellSettingsSchema>;

/** The `quit-requested` event (#29): quitting would stop this many downloads, so the window asks first. */
export const QuitRequestSchema = z.strictObject({ active: z.number().int().nonnegative() });
export type QuitRequest = z.infer<typeof QuitRequestSchema>;

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
