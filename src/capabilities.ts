// What this build can do (#57): the engine reports its capabilities per protocol, and the UI never
// offers what isn't there. A control for something missing is disabled, with the reason beside
// it. #30's Settings screen and the download details (R8) place these pieces
// (`CapabilityOption.tsx`).
import type { Capabilities, DownloadKind, ProtocolCapabilities } from './contract';

/** A feature of one protocol, as `ProtocolCapabilities` reports it. */
export type ProtocolFeature = keyof Omit<ProtocolCapabilities, 'kind'>;

/** Whether something is available, and if not, why, in words for the user. */
export interface Availability {
  available: boolean;
  reason: string | null;
}

const AVAILABLE: Availability = { available: true, reason: null };

/** What the build can't do when it doesn't list a protocol at all. */
const MISSING: Record<DownloadKind, string> = {
  http: 'This version of Velocity can’t download over HTTP yet.',
  ftp: 'This version of Velocity can’t download over FTP yet.',
  sftp: 'This version of Velocity can’t download over SFTP yet.',
  metalink: 'This version of Velocity can’t open Metalink files yet.',
  torrent: 'This version of Velocity can’t download torrents yet.',
  magnet: 'This version of Velocity can’t open magnet links yet.',
};

/** A protocol's name in a sentence. */
const NAME: Record<DownloadKind, string> = {
  http: 'HTTP',
  ftp: 'FTP',
  sftp: 'SFTP',
  metalink: 'Metalink',
  torrent: 'BitTorrent',
  magnet: 'BitTorrent',
};

/** What a protocol lacks when it doesn't do `feature`. */
function lacking(kind: DownloadKind, feature: ProtocolFeature): string {
  const name = NAME[kind];
  switch (feature) {
    case 'segmented':
      return `Downloads over ${name} use one connection in this version of Velocity.`;
    case 'resume':
      return `Downloads over ${name} start over after a pause in this version of Velocity.`;
    case 'validatorResume':
      return `A resumed download over ${name} isn’t checked against the server’s version of the file.`;
    case 'http2':
      return kind === 'http' ? 'This version of Velocity doesn’t use HTTP/2.' : 'HTTP/2 is only for HTTP and HTTPS.';
  }
}

/** Whether this build downloads `kind`, and, given `feature`, whether it does that for it. */
export function availability(capabilities: Capabilities, kind: DownloadKind, feature?: ProtocolFeature): Availability {
  const protocol = capabilities.protocols.find((p) => p.kind === kind);
  if (!protocol) return { available: false, reason: MISSING[kind] };
  if (feature && !protocol[feature]) return { available: false, reason: lacking(kind, feature) };
  return AVAILABLE;
}
