import branding from '../../../branding.json';

/**
 * Set by the build (`scripts/build-extension.mjs --release`): a release build talks to the release
 * app's native host, a development build to the development app's. Tests leave it undefined, and
 * get the development names.
 */
declare const __VELOCITY_RELEASE__: boolean | undefined;

const release = typeof __VELOCITY_RELEASE__ === 'boolean' && __VELOCITY_RELEASE__;

export const NATIVE_HOST_NAME: string = release ? branding.releaseNativeHostName : branding.nativeHostName;
export const EXTENSION_NAME: string = release ? branding.releaseExtensionName : branding.extensionName;
/** "Velocity V2": what menus, badges and messages call the app, so it can't be mistaken for V1. */
export const SHORT_NAME: string = branding.extensionShortName;
