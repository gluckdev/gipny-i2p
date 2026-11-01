/** Main-thread WebAuthn PRF helper.
 * Generates and evaluates cryptographic passkey seeds using the W3C WebAuthn
 * Level 3 PRF (hmac-secret) extension. Private PRF seeds are used by Vault
 * to derive profile encryption keys and must be zeroized after use. */

function toUint8Array(source: BufferSource): Uint8Array {
  if (source instanceof ArrayBuffer) {
    return new Uint8Array(source);
  }
  return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
}

function toBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < bytes.byteLength; i++) {
    const byte = bytes[i];
    if (byte !== undefined) {
      binary += String.fromCharCode(byte);
    }
  }
  return btoa(binary)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}

function arraysEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function formatWebAuthnError(error: unknown, stage: string): Error {
  if (error instanceof DOMException) {
    switch (error.name) {
      case 'NotAllowedError':
        return new Error(`${stage} was cancelled or timed out`);
      case 'NotSupportedError':
        return new Error(`${stage} is not supported by this authenticator or browser`);
      case 'InvalidStateError':
        return new Error(`${stage}: authenticator was already registered or is in an invalid state`);
      case 'SecurityError':
        return new Error(`${stage} failed security verification (origin or certificate mismatch)`);
      case 'AbortError':
        return new Error(`${stage} was aborted`);
      default:
        return new Error(`${stage} failed (${error.name}): ${error.message}`);
    }
  }
  if (error instanceof Error) {
    return error;
  }
  return new Error(`${stage} failed: ${String(error)}`);
}

function ensureOrigin(rpId?: string): string {
  if (typeof window === 'undefined' || !window.location?.hostname) {
    throw new Error('Passkey operations require a browser window context');
  }
  if (!window.isSecureContext) {
    throw new Error('WebAuthn requires a secure origin (HTTPS or localhost)');
  }
  if (!navigator.credentials?.create || !navigator.credentials?.get) {
    throw new Error('This browser does not support WebAuthn');
  }
  const hostname = window.location.hostname;
  if (!hostname) {
    throw new Error('WebAuthn requires a valid origin hostname');
  }
  if (rpId && rpId !== hostname) {
    throw new Error(`Invalid rpId "${rpId}": must match current origin hostname "${hostname}"`);
  }
  return hostname;
}

export async function enrollBrowserPasskey(
  profileName: string,
  rpId?: string,
): Promise<{ credentialId: Uint8Array; rpId: string; salt: Uint8Array; prf: Uint8Array }> {
  const effectiveRpId = ensureOrigin(rpId);

  const name = profileName.normalize('NFC').trim();
  if (!name || new TextEncoder().encode(name).length > 128) {
    throw new Error('Invalid profile name');
  }

  const createChallenge = crypto.getRandomValues(new Uint8Array(32));
  const userId = crypto.getRandomValues(new Uint8Array(32));

  let credential: PublicKeyCredential;
  try {
    const cred = await navigator.credentials.create({
      publicKey: {
        challenge: createChallenge,
        rp: {
          id: effectiveRpId,
          name: 'Gipny Browser',
        },
        user: {
          id: userId,
          name,
          displayName: name,
        },
        pubKeyCredParams: [
          { alg: -7, type: 'public-key' },   // ES256
          { alg: -257, type: 'public-key' }, // RS256
        ],
        authenticatorSelection: {
          userVerification: 'required',
          residentKey: 'preferred',
        },
        extensions: {
          prf: {},
        },
      },
    });

    if (!cred || !(cred instanceof PublicKeyCredential)) {
      throw new Error('Passkey creation returned invalid response');
    }
    credential = cred;
  } catch (error) {
    throw formatWebAuthnError(error, 'Passkey registration');
  }

  const createOutputs = credential.getClientExtensionResults();
  if (!createOutputs.prf?.enabled) {
    throw new Error('This passkey does not support the WebAuthn PRF extension required for vault encryption');
  }

  const credentialId = new Uint8Array(credential.rawId);
  const salt = crypto.getRandomValues(new Uint8Array(32));
  const assertChallenge = crypto.getRandomValues(new Uint8Array(32));

  let assertion: PublicKeyCredential;
  try {
    const assertCred = await navigator.credentials.get({
      publicKey: {
        challenge: assertChallenge,
        rpId: effectiveRpId,
        allowCredentials: [
          {
            id: credentialId,
            type: 'public-key',
          },
        ],
        userVerification: 'required',
        extensions: {
          prf: {
            eval: {
              first: salt,
            },
          },
        },
      },
    });

    if (!assertCred || !(assertCred instanceof PublicKeyCredential)) {
      throw new Error('Passkey evaluation returned invalid response');
    }
    assertion = assertCred;
  } catch (error) {
    throw formatWebAuthnError(error, 'Passkey evaluation');
  }

  const assertOutputs = assertion.getClientExtensionResults();
  const rawPrf = assertOutputs.prf?.results?.first;
  if (!rawPrf) {
    throw new Error('Passkey did not return PRF extension output');
  }

  const prf = toUint8Array(rawPrf);
  if (prf.byteLength !== 32) {
    throw new Error(`Invalid PRF output length: expected 32 bytes, got ${prf.byteLength}`);
  }

  return {
    credentialId,
    rpId: effectiveRpId,
    salt,
    prf,
  };
}

export async function unlockBrowserPasskey(
  entries: Array<{ credentialId: Uint8Array; rpId: string; salt: Uint8Array }>,
): Promise<{ credentialId: Uint8Array; prf: Uint8Array }> {
  if (typeof window === 'undefined' || !window.location?.hostname) {
    throw new Error('Passkey operations require a browser window context');
  }
  if (!window.isSecureContext) {
    throw new Error('WebAuthn requires a secure origin (HTTPS or localhost)');
  }
  if (!navigator.credentials?.get) {
    throw new Error('This browser does not support WebAuthn');
  }

  const hostname = window.location.hostname;
  if (!hostname) {
    throw new Error('WebAuthn requires a valid origin hostname');
  }
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new Error('No passkey credentials available for this profile');
  }

  const matching = entries.filter(e => e.rpId === hostname && e.salt.byteLength === 32 && e.credentialId.byteLength > 0);
  if (matching.length === 0) {
    throw new Error(`No passkeys found for current origin (${hostname})`);
  }

  const allowCredentials: PublicKeyCredentialDescriptor[] = matching.map(entry => ({
    id: new Uint8Array(entry.credentialId),
    type: 'public-key',
  }));

  const evalByCredential: Record<string, AuthenticationExtensionsPRFValues> = {};
  for (const entry of matching) {
    evalByCredential[toBase64Url(entry.credentialId)] = {
      first: new Uint8Array(entry.salt),
    };
  }

  const firstEntry = matching[0];
  if (!firstEntry) {
    throw new Error('No valid passkey entries');
  }

  const challenge = crypto.getRandomValues(new Uint8Array(32));

  let assertion: PublicKeyCredential;
  try {
    const cred = await navigator.credentials.get({
      publicKey: {
        challenge,
        rpId: hostname,
        allowCredentials,
        userVerification: 'required',
        extensions: {
          prf: {
            eval: {
              first: new Uint8Array(firstEntry.salt),
            },
            evalByCredential,
          },
        },
      },
    });

    if (!cred || !(cred instanceof PublicKeyCredential)) {
      throw new Error('Passkey authentication returned invalid response');
    }
    assertion = cred;
  } catch (error) {
    throw formatWebAuthnError(error, 'Passkey unlock');
  }

  const selectedId = new Uint8Array(assertion.rawId);
  const matched = matching.find(e => arraysEqual(e.credentialId, selectedId));
  if (!matched) {
    throw new Error('Passkey credential ID does not match any enrolled passkey');
  }

  const outputs = assertion.getClientExtensionResults();
  const rawPrf = outputs.prf?.results?.first;
  if (!rawPrf) {
    throw new Error('Passkey did not return PRF extension output');
  }

  const prf = toUint8Array(rawPrf);
  if (prf.byteLength !== 32) {
    throw new Error(`Invalid PRF output length: expected 32 bytes, got ${prf.byteLength}`);
  }

  return {
    credentialId: matched.credentialId,
    prf,
  };
}
