import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { chromeExtensionId, manifest, OPTIONAL_HOST_PERMISSIONS, PERMISSIONS } from './extension-lib.mjs';

const branding = JSON.parse(readFileSync(new URL('../branding.json', import.meta.url), 'utf8'));
/** V1's Chrome key and the ID it pins (V1 `branding.json` at e9d717a8). */
const V1_KEY =
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAo15wOoNApvSfuGVevsk7fqI3qo1KYF24IG1NWijAmrFWo0FtL1fVRXq5LZsVyQN1bkuobZufFxPArf0B+CmXOfqkOzAeBAp/ZZPTRjFoIT5kOMtx/0MH63N1/+r5kpAYkmFqWTtukdjCiD3mKZGrinSpjbg2bOYV3RbvyIlH94MsQt8ANDWOx7ffQ+h8eGLYUNhrIWjtPT+tOnKnRPXxyQJjb+j3IiNINZ6PfSKpOiWTILdLdmOyg5QQdgXTFwPQ+YPMAsQ8Bf+TlvlaDgzGnEsvwKek0O2m2VdZ9dtBo/tP+t6VBGO4CkU+zi0SHGO2ZjM8yrPkGe81jbmFQ1/EnwIDAQAB';
const V1_ID = 'eecopkgfiffjgmngfmjmdaejbkbbippo';

describe('chromeExtensionId', () => {
  it("derives the ID Chrome gives a key (checked against V1's known pair)", () => {
    expect(chromeExtensionId(V1_KEY)).toBe(V1_ID);
  });

  it("matches the ID that branding.json lists for V2's key, which isn't V1's", () => {
    expect(branding.chromeExtensionIds).toEqual([chromeExtensionId(branding.chromeExtensionKey)]);
    expect(branding.chromeExtensionIds).not.toContain(V1_ID);
  });
});

describe('manifest', () => {
  const dev = (target) => manifest(target, { release: false, version: '0.1.0', branding });

  it('asks for the same minimum permissions as V1, with page access optional', () => {
    for (const target of ['chrome', 'firefox']) {
      expect(dev(target).permissions).toEqual(PERMISSIONS);
      expect(dev(target).optional_host_permissions).toEqual(OPTIONAL_HOST_PERMISSIONS);
      expect(dev(target)).not.toHaveProperty('host_permissions');
      expect(dev(target).permissions).not.toContain('cookies');
    }
  });

  it("pins V2's Chrome key and a service worker for Chromium", () => {
    const m = dev('chrome');
    expect(m.key).toBe(branding.chromeExtensionKey);
    expect(m.background).toEqual({ service_worker: 'background.js' });
    expect(m).not.toHaveProperty('browser_specific_settings');
  });

  it("pins V2's Firefox ID and a background script for Firefox", () => {
    const m = dev('firefox');
    expect(m.browser_specific_settings.gecko.id).toBe(branding.firefoxExtensionId);
    expect(m.background).toEqual({ scripts: ['background.js'] });
    expect(m).not.toHaveProperty('key');
  });

  it('names the development and release builds apart, both with V2', () => {
    expect(dev('chrome').name).toBe(branding.extensionName);
    expect(manifest('chrome', { release: true, version: '0.1.0', branding }).name).toBe(branding.releaseExtensionName);
    expect(dev('chrome').name).toMatch(/V2/);
  });

  it('runs only its own scripts on its pages', () => {
    expect(dev('chrome').content_security_policy.extension_pages).toBe("script-src 'self'; object-src 'none'; base-uri 'none'");
  });
});
