'use strict';
// Never print, persist, generate, or accept private key material on the command line.
const fs = require('node:fs');
const path = require('node:path');
const { createPrivateKey, createPublicKey, sign, verify } = require('node:crypto');
const CONTEXT = Buffer.from('zstock-update-v1\n');
const SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');
function publicKeyFromHex(text) {
  const hex = text.trim();
  if (!/^[a-f0-9]{64}$/i.test(hex)) throw new Error('Missing valid owner-provided Ed25519 public key; publication is blocked');
  return createPublicKey({ key: Buffer.concat([SPKI_PREFIX, Buffer.from(hex, 'hex')]), format: 'der', type: 'spki' });
}
function publicKeyHex(key) {
  const publicKey = createPublicKey(key);
  if (publicKey.asymmetricKeyType !== 'ed25519') throw new Error('Expected an Ed25519 key');
  const der = publicKey.export({ format: 'der', type: 'spki' });
  if (der.length !== 44 || !der.subarray(0, 12).equals(SPKI_PREFIX)) throw new Error('Invalid Ed25519 public key');
  return der.subarray(12).toString('hex');
}
function signManifest(payload, privatePem, publicHex) {
  if (!privatePem) throw new Error('ZSTOCK_UPDATE_SIGNING_KEY is missing; publication is blocked');
  let key;
  try { key = createPrivateKey(privatePem); } catch { throw new Error('Invalid Ed25519 PKCS#8 signing key'); }
  if (key.asymmetricKeyType !== 'ed25519' || publicKeyHex(key) !== publicHex.trim().toLowerCase()) {
    throw new Error('Signing key does not match the embedded release public key');
  }
  const signature = sign(null, Buffer.concat([CONTEXT, Buffer.from(payload)]), key).toString('base64');
  const envelope = JSON.stringify({ schema: 1, payload, signature }, null, 2) + '\n';
  verifyManifest(envelope, publicHex);
  return envelope;
}
function verifyManifest(envelope, publicHex) {
  const parsed = JSON.parse(envelope);
  if (Buffer.byteLength(envelope) > 128 * 1024 || parsed.schema !== 1 || typeof parsed.payload !== 'string'
      || typeof parsed.signature !== 'string' || !/^[A-Za-z0-9+/]{86}==$/.test(parsed.signature)
      || Object.keys(parsed).sort().join(',') !== 'payload,schema,signature') throw new Error('Invalid signed update envelope');
  const signature = Buffer.from(parsed.signature, 'base64');
  if (!verify(null, Buffer.concat([CONTEXT, Buffer.from(parsed.payload)]), publicKeyFromHex(publicHex), signature)) {
    throw new Error('Update manifest signature verification failed');
  }
  return JSON.parse(parsed.payload);
}
if (require.main === module) {
  try {
    const root = path.resolve(__dirname, '..');
    if (process.argv[2] === '--public-key') {
      // Public-only conversion for the owner-provided PEM; reject private PEMs.
      const pem = fs.readFileSync(process.argv[3], 'utf8');
      if (!pem.startsWith('-----BEGIN PUBLIC KEY-----')) throw new Error('Only PUBLIC KEY PEM is accepted');
      const key = createPublicKey(pem);
      if (key.asymmetricKeyType !== 'ed25519') throw new Error('Expected Ed25519 public key');
      const der = key.export({ format: 'der', type: 'spki' });
      if (der.length !== 44 || !der.subarray(0, 12).equals(SPKI_PREFIX)) throw new Error('Invalid public key encoding');
      process.stdout.write(der.subarray(12).toString('hex') + '\n');
    } else {
      const payload = fs.readFileSync(path.join(root, 'updates/update-payload.json'), 'utf8');
      const publicHex = fs.readFileSync(path.join(root, 'updates/signing-public-key.hex'), 'utf8');
      const envelope = signManifest(payload, process.env.ZSTOCK_UPDATE_SIGNING_KEY, publicHex);
      fs.writeFileSync(path.join(root, 'updates/stable-v2.json'), envelope);
      fs.writeFileSync(path.join(root, 'release-assets/zstock-update-manifest.json'), envelope);
      console.log('Verified signed update manifest written');
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
module.exports = { signManifest, verifyManifest, publicKeyFromHex, publicKeyHex };
