// WebDriver's Virtual Authenticators (https://w3c.github.io/webauthn/#sctn-automation) over CDP's WebAuthn domain, the
// way chromedriver does: add_virtual_authenticator -> WebAuthn.addVirtualAuthenticator, add_credential, get_credentials,
// remove_credential, remove_all_credentials, set_user_verified, set_credential_properties, remove_virtual_authenticator.
// The authenticators live in the browser context (pyro-webapi `webauthn`), so every page and frame of it sees them.
// WebDriver sends binary values as base64url, CDP as base64.

const PROTOCOLS = {
  'ctap1/u2f': { protocol: 'u2f' },
  ctap2: { protocol: 'ctap2', ctap2Version: 'ctap2_0' },
  ctap2_1: { protocol: 'ctap2', ctap2Version: 'ctap2_1' },
};
const TRANSPORTS = { usb: 'usb', nfc: 'nfc', ble: 'ble', hybrid: 'cable', internal: 'internal' };

const toCdp = (b64url) => Buffer.from(String(b64url), 'base64url').toString('base64');
const fromCdp = (b64) => Buffer.from(b64, 'base64').toString('base64url');

// A CDP credential as WebDriver's "Get Credentials" returns it.
function webdriverCredential(c) {
  const out = { credentialId: fromCdp(c.credentialId), isResidentCredential: c.isResidentCredential, rpId: c.rpId, privateKey: fromCdp(c.privateKey), signCount: c.signCount };
  if (c.userHandle !== undefined) out.userHandle = fromCdp(c.userHandle);
  if (c.largeBlob !== undefined) out.largeBlob = fromCdp(c.largeBlob);
  for (const k of ['backupEligibility', 'backupState', 'userName', 'userDisplayName']) if (c[k] !== undefined) out[k] = c[k];
  return out;
}

async function webauthn(ctx, context, method, params = {}) {
  const page = context ? await ctx.windowPage(context) : ctx.page;
  if (!page) throw new Error('no such window');
  const session = page === ctx.page ? await ctx.session() : await ctx.sessionOf(page);
  await session.send('WebAuthn.enable');
  return session.send(`WebAuthn.${method}`, params);
}

export const commands = {
  async add_virtual_authenticator(ctx, [config, context]) {
    const base = PROTOCOLS[config?.protocol];
    if (!base) throw new Error(`invalid argument: unknown protocol ${config?.protocol}`);
    const transport = TRANSPORTS[config.transport];
    if (!transport) throw new Error(`invalid argument: unknown transport ${config.transport}`);
    const extensions = config.extensions ?? [];
    const options = {
      ...base,
      transport,
      hasResidentKey: !!config.hasResidentKey,
      hasUserVerification: !!config.hasUserVerification,
      hasLargeBlob: extensions.includes('largeBlob'),
      hasCredBlob: extensions.includes('credBlob'),
      hasMinPinLength: extensions.includes('minPinLength'),
      hasPrf: extensions.includes('prf'),
      // WebDriver's isUserConsenting: whether a user touches the authenticator when a request needs one.
      automaticPresenceSimulation: config.isUserConsenting ?? true,
      isUserVerified: !!config.isUserVerified,
    };
    if (config.defaultBackupEligibility !== undefined) options.defaultBackupEligibility = !!config.defaultBackupEligibility;
    if (config.defaultBackupState !== undefined) options.defaultBackupState = !!config.defaultBackupState;
    return (await webauthn(ctx, context, 'addVirtualAuthenticator', { options })).authenticatorId;
  },
  async remove_virtual_authenticator(ctx, [authenticatorId, context]) {
    await webauthn(ctx, context, 'removeVirtualAuthenticator', { authenticatorId });
  },
  async add_credential(ctx, [authenticatorId, credential, context]) {
    const c = { ...credential, credentialId: toCdp(credential.credentialId), privateKey: toCdp(credential.privateKey) };
    if (credential.userHandle !== undefined) c.userHandle = toCdp(credential.userHandle);
    if (credential.largeBlob !== undefined) c.largeBlob = toCdp(credential.largeBlob);
    await webauthn(ctx, context, 'addCredential', { authenticatorId, credential: c });
  },
  async get_credentials(ctx, [authenticatorId, context]) {
    return (await webauthn(ctx, context, 'getCredentials', { authenticatorId })).credentials.map(webdriverCredential);
  },
  async remove_credential(ctx, [authenticatorId, credentialId, context]) {
    await webauthn(ctx, context, 'removeCredential', { authenticatorId, credentialId: toCdp(credentialId) });
  },
  async remove_all_credentials(ctx, [authenticatorId, context]) {
    await webauthn(ctx, context, 'clearCredentials', { authenticatorId });
  },
  async set_user_verified(ctx, [authenticatorId, uv, context]) {
    await webauthn(ctx, context, 'setUserVerified', { authenticatorId, isUserVerified: !!uv?.isUserVerified });
  },
  async set_credential_properties(ctx, [authenticatorId, credentialId, props, context]) {
    await webauthn(ctx, context, 'setCredentialProperties', { authenticatorId, credentialId: toCdp(credentialId), ...props });
  },
};
