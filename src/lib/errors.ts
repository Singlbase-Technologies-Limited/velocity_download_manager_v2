import { ErrorCodeSchema, type ErrorCode } from '../contract';

/**
 * What each error code means for the user: the sentences Rust gives them (`ErrorCode::user_message`),
 * which a test keeps the same as the generated fixture `error-codes.json`.
 */
export const ERROR_MESSAGES: Record<ErrorCode, string> = {
  INVALID_INPUT: "Some of the values entered aren't valid.",
  INVALID_URL: "The link isn't a valid download address.",
  UNSUPPORTED_SCHEME: "This kind of link can't be downloaded.",
  INVALID_PATH: "The folder path isn't valid.",
  INVALID_FILENAME: "The file name isn't allowed on this system.",
  DESTINATION_MISSING: "The download folder doesn't exist.",
  PERMISSION_DENIED: "Velocity isn't allowed to write to that location.",
  FILE_EXISTS: "A file with that name already exists.",
  NOT_FOUND: "That download or file no longer exists.",
  NAME_TAKEN: "That name is already taken.",
  INVALID_STATE: "That action isn't possible in the download's current state.",
  ENGINE_MISSING: "The transfer engine is missing from this installation.",
  ENGINE_INCOMPATIBLE: "The transfer engine doesn't match this version of Velocity.",
  ENGINE_START_FAILED: "The transfer engine couldn't be started.",
  ENGINE_UNAVAILABLE: "The transfer engine isn't responding.",
  DATABASE: "Velocity couldn't read or save its data.",
  DATABASE_MIGRATION: "Velocity couldn't upgrade its data to this version.",
  IO: "A file couldn't be read or written.",
  INSUFFICIENT_DISK_SPACE: "There isn't enough free disk space.",
  PROTOCOL_DISABLED: "This kind of download is turned off in Settings.",
  CONFIRMATION_REQUIRED: "This action needs your confirmation first.",
  UNSUPPORTED: "This isn't supported on this system yet.",
  INTERNAL: "Something went wrong inside Velocity.",
};

/** A failed command: its code, the sentence for the user, and any detail for diagnostics. */
export class ApiError extends Error {
  constructor(
    readonly code: ErrorCode,
    message: string,
    readonly detail: string | null = null,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** Whatever a command failed with, as an ApiError. Anything that isn't one of the codes is `INTERNAL`, keeping what it was. */
export function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  const code = ErrorCodeSchema.safeParse(e);
  if (code.success) return new ApiError(code.data, ERROR_MESSAGES[code.data]);
  const detail = e instanceof Error ? e.message : String(e);
  return new ApiError('INTERNAL', ERROR_MESSAGES.INTERNAL, detail);
}
