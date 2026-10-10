# Release update trust (0.0.60+)

## Trust bootstrap

ZStock 0.0.59 and earlier cannot authenticate update manifests. A signature added
by a later release cannot retroactively fix that client. Install the bootstrap
release manually from <https://github.com/csic21/zstock/releases>; verify the
published package digest before installation. Do not use the old in-app updater
to establish the first trusted key. This is a trust-on-manual-install boundary.

The legacy `updates/stable.json` is a manual-install-only notice with no platform
packages. Old clients stop with a missing-platform message instead of executing
an unsigned upgrade. New clients exclusively consume `updates/stable-v2.json`.

If `updates/signing-public-key.hex` is empty or malformed, automatic updates fail
closed with an official manual-download URL. An empty file is not a trust anchor.
Release publication also fails until the owner provides a real key and matching
GitHub Actions secret. No test key may be used for production.

## Owner-controlled setup

The owner creates an Ed25519 key on a trusted computer, in a private directory:

```sh
umask 077
openssl genpkey -algorithm ED25519 -out zstock-update-private.pem
openssl pkey -in zstock-update-private.pem -pubout -out zstock-update-public.pem
```

Keep the private key and a secure backup private. Store its complete PKCS#8 PEM
(`BEGIN PRIVATE KEY`) in repository Actions secret `ZSTOCK_UPDATE_SIGNING_KEY`
using GitHub's secure Settings UI. Never paste private material in a chat, issue,
commit, command argument, screenshot, artifact, or log.

The public PEM (`BEGIN PUBLIC KEY`) is safe to share. Validate and convert it with:

```sh
openssl pkey -pubin -in zstock-update-public.pem -text -noout
node scripts/sign-update.cjs --public-key zstock-update-public.pem > updates/signing-public-key.hex
```

Review the public-key change separately. The updater embeds these exact 32 bytes
at build time. A manifest, mirror, environment variable, or downloaded key cannot
replace them. Rotation requires a deliberately reviewed trusted-client release
or another manual installation; there is no automatic network key enrollment.

## Signed contract

`stable-v2.json` is an envelope with exactly `schema`, `payload`, and `signature`.
`schema` is 1; `payload` is an exact UTF-8 JSON string. Ed25519 signs the bytes of
`zstock-update-v1\n` followed by the payload, with no JSON normalization. The
signature is standard base64. The payload binds version, release URL, all four
platform ZIP URLs and their mandatory SHA-256 digests. This authenticates each
package through its signed digest, independently of the hosting/mirror account.

The client verifies the signature before considering a version, requires a newer
stable semver, and accepts only the exact versioned csic21/zstock GitHub ZIP URL
for its platform. Downloads may redirect only to GitHub's HTTPS asset CDN; the
manifest endpoints cannot redirect. Size limits, timeouts, ZIP path/symlink checks
and mandatory package hash verification precede extraction/installation.

The workflow signs only after all native quality gates and nine platform packages
are available. `publish-release.cjs` independently verifies the pinned signature,
package digests, tag/source identity, and unchanged publication head before any
remote publication. The signed envelope is also uploaded as the tenth release
asset, `zstock-update-manifest.json`, and its exact uploaded digest is verified.
The two repository manifests advance in one non-force commit after publication.
A race may leave a published release with the old feed; rerunning is safe and
cannot overwrite newer main history. No commit dates are added for this release.

## Limits

A compromised signing key or a reviewed build containing malicious code defeats
this trust model. GitHub Actions' secret remains available to authorized release
workflow code; protect that repository and review workflow changes. Signing does
not make the legacy client trustworthy and does not establish Apple notarization
or Microsoft Authenticode. macOS packages remain ad-hoc signed unless separately
provisioned by the owner. Signature checks stop substituted updates; they do not
prove a running app will be responsive or that market data is current.

The fixture public/private vector in tests is published RFC 8032 material. It is
used only in disposable offline tests and never configured as the production key.

## Bootstrap verification key

The owner supplied the Ed25519 public SPKI on 2026-10-10 and confirmed configuring
`ZSTOCK_UPDATE_SIGNING_KEY` in the secure repository UI. Its SPKI SHA-256 is
`cb1d9cb818d1e22e7a9f3fb8a3f5e58324533e5e03faa9935632a68bbdcc561b`.
The workflow must still prove that the secret matches the compiled public key
before publishing; configuration confirmation alone is not cryptographic proof.
