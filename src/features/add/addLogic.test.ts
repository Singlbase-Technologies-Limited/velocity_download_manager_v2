import { describe, expect, it } from 'vitest';
import type { Capabilities } from '../../contract';
import { fixture, withSftp as addSftp } from '../../test/fakeBackend';
import { isTorrentLink, linkKind, linkProblem, parseChecksum, parseHeaders, splitLinks } from './addLogic';

// Ported from V1's tests/unit/addLogic.test.ts, with V2's SFTP links and engine capabilities (#60).
const capabilities = fixture('capabilities.json') as Capabilities;
const withSftp = addSftp(capabilities);

describe('add download input parsing', () => {
  it('splits pasted links on whitespace and removes duplicates', () => {
    expect(splitLinks(' https://a.test/1\n\nhttps://b.test/2 https://a.test/1\r\n')).toEqual(['https://a.test/1', 'https://b.test/2']);
    expect(splitLinks('   ')).toEqual([]);
  });

  it('accepts supported schemes and rejects others, saying why', () => {
    expect(linkProblem('https://example.com/f', withSftp)).toBeNull();
    expect(linkProblem('ftp://ftp.example.org/pub/1MB.zip', withSftp)).toBeNull();
    expect(linkProblem('ftps://ftp.example.org/pub/1MB.zip', withSftp)).toBeNull();
    expect(linkProblem('sftp://example.com/f.zip', withSftp)).toBeNull();
    expect(linkProblem('magnet:?xt=urn:btih:abc', withSftp)).toBeNull();
    expect(linkProblem('file:///etc/passwd', withSftp)).toMatch(/Only http, https, ftp, ftps, sftp and magnet links/);
    expect(linkProblem('javascript:alert(1)', withSftp)).toMatch(/Only http, https, ftp, ftps, sftp and magnet links/);
    expect(linkProblem('example.com/file', withSftp)).toMatch(/not a complete link/);
    expect(linkProblem('ftp://', withSftp)).toMatch(/not a complete link/);
  });

  it('asks for a login in its own fields, never in the link', () => {
    expect(linkProblem('https://me:secret@example.com/a.zip', withSftp)).toMatch(/user name and password in their own fields/);
  });

  it('says when this build can’t download a protocol, before anything is sent', () => {
    // The sample engine lists no SFTP.
    expect(linkProblem('sftp://example.com/f.zip', capabilities)).toBe('This version of Velocity can’t download over SFTP yet.');
    // Until the capabilities arrive, only the native side can say.
    expect(linkProblem('sftp://example.com/f.zip', undefined)).toBeNull();
  });

  it('tells the kinds of link apart', () => {
    expect(linkKind('HTTPS://e.com/a')).toBe('http');
    expect(linkKind('ftps://e.com/a')).toBe('ftp');
    expect(linkKind('sftp://e.com/a')).toBe('sftp');
    expect(linkKind('Magnet:?xt=urn:btih:aa')).toBe('magnet');
    expect(linkKind('https://e.com/album.torrent')).toBe('torrentLink');
    expect(linkKind('mailto:me@e.com')).toBeNull();
  });

  it('tells .torrent links apart as the native side does', () => {
    // The same cases as `torrent_links_are_told_apart_as_v1_did` in Rust.
    expect(isTorrentLink('https://e.com/a/album.TORRENT')).toBe(true);
    expect(isTorrentLink('http://e.com/album.torrent?x=1#y')).toBe(true);
    expect(isTorrentLink('https://e.com/download.php?f=album.torrent')).toBe(false);
    expect(isTorrentLink('ftp://e.com/album.torrent')).toBe(false);
    expect(isTorrentLink('magnet:?xt=urn:btih:aa')).toBe(false);
    expect(isTorrentLink('https://e.com/album.torrent.zip')).toBe(false);
  });

  it('parses headers and reports malformed lines', () => {
    expect(parseHeaders('Cookie: a=b\n  Accept : */*  \n')).toEqual({
      headers: [
        { name: 'Cookie', value: 'a=b' },
        { name: 'Accept', value: '*/*' },
      ],
      error: null,
    });
    expect(parseHeaders('Cookie a=b').error).toMatch(/Line 1/);
    expect(parseHeaders(': nothing').error).toMatch(/Line 1/);
  });

  it('parses a checksum as V1 wrote it, and checks its length', () => {
    const hex = 'AB'.repeat(32);
    expect(parseChecksum('')).toEqual({ checksum: null, error: null });
    expect(parseChecksum(`sha-256=${hex}`)).toEqual({ checksum: { algorithm: 'sha256', hex: hex.toLowerCase() }, error: null });
    expect(parseChecksum(`SHA256:${hex}`).checksum?.algorithm).toBe('sha256');
    expect(parseChecksum(`md5=${'0'.repeat(32)}`).checksum?.algorithm).toBe('md5');
    expect(parseChecksum(`sha-256=${'ab'.repeat(20)}`).error).toMatch(/64 hexadecimal/);
    expect(parseChecksum(`crc32=${'ab'.repeat(4)}`).error).toMatch(/md5, sha-1/);
    expect(parseChecksum(hex).error).toMatch(/like “sha-256=/);
  });
});
