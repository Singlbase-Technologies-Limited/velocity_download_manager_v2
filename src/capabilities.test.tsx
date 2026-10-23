import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { availability } from './capabilities';
import { CapabilityOption } from './CapabilityOption';
import type { Capabilities } from './contract';

// Like the engine's report today: HTTP(S) only, segmented, resumable, over HTTP/2.
const caps: Capabilities = {
  protocols: [{ kind: 'http', segmented: true, resume: true, validatorResume: true, http2: true }],
  liveConnectionChanges: true,
  metalink: false,
  maxConnectionsPerHost: 8,
};
const withFtp: Capabilities = {
  ...caps,
  protocols: [...caps.protocols, { kind: 'ftp', segmented: false, resume: true, validatorResume: false, http2: false }],
};

describe('availability', () => {
  it('a protocol the engine lists is available', () => {
    expect(availability(caps, 'http')).toEqual({ available: true, reason: null });
    expect(availability(caps, 'http', 'segmented')).toEqual({ available: true, reason: null });
  });

  it('a protocol the engine doesn’t list is unavailable, saying so', () => {
    expect(availability(caps, 'sftp')).toEqual({
      available: false,
      reason: 'This version of Velocity can’t download over SFTP yet.',
    });
    expect(availability(caps, 'torrent').reason).toBe('This version of Velocity can’t download torrents yet.');
  });

  it('a feature a protocol lacks is unavailable, saying which', () => {
    expect(availability(withFtp, 'ftp', 'segmented')).toEqual({
      available: false,
      reason: 'Downloads over FTP use one connection in this version of Velocity.',
    });
    expect(availability(withFtp, 'ftp', 'http2').reason).toBe('HTTP/2 is only for HTTP and HTTPS.');
    // A feature of a protocol that isn't there gives the protocol's reason.
    expect(availability(caps, 'sftp', 'resume').reason).toBe('This version of Velocity can’t download over SFTP yet.');
  });
});

describe('CapabilityOption', () => {
  it('an unavailable option is disabled, with its reason tied to it', () => {
    const html = renderToStaticMarkup(
      <CapabilityOption label="Use several connections" availability={availability(withFtp, 'ftp', 'segmented')} checked={false} />,
    );
    expect(html).toMatch(/<input[^>]*disabled=""/);
    expect(html).toContain('Downloads over FTP use one connection in this version of Velocity.');
    const described = /aria-describedby="([^"]+)"/.exec(html)?.[1];
    expect(described).toBeTruthy();
    expect(html).toContain(`id="${described ?? ''}"`);
  });

  it('an available option is enabled, with no reason', () => {
    const html = renderToStaticMarkup(
      <CapabilityOption label="Use several connections" availability={availability(caps, 'http', 'segmented')} checked />,
    );
    expect(html).not.toContain('disabled');
    expect(html).not.toContain('aria-describedby');
    expect(html).toContain('Use several connections');
  });
});
