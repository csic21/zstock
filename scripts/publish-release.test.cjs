'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { createHash } = require('node:crypto');
const { ASSET_NAMES, commitIdentity, validateManifest, verifyAsset, publishRelease } = require('./publish-release.cjs');
const overrides = require('../.github/release-commit-dates.json');
const SOURCE = 'a'.repeat(40);
const HEAD = 'b'.repeat(40);
const parent = () => ({ author: { date: '2026-10-09T00:23:00Z' }, committer: { date: '2026-10-09T00:24:00Z' },
  parents: [{ sha: SOURCE }], tree: { sha: 'tree' } });

test('only v0.0.58 receives the explicitly bounded author and committer date', () => {
  const identity = commitIdentity('v0.0.58', overrides, parent(), parent());
  assert.equal(identity.author.date, '2026-10-09T08:59:00+08:00');
  assert.deepEqual(identity.author, identity.committer);
  assert.deepEqual(commitIdentity('v0.0.59', overrides, parent(), parent()), {});
  assert.deepEqual(Object.keys(overrides), ['v0.0.58']);
});

test('override must follow both source and parent author/committer dates', () => {
  for (const role of ['author', 'committer']) {
    const later = parent(); later[role].date = '2026-10-09T01:00:00Z';
    assert.throws(() => commitIdentity('v0.0.58', overrides, later, parent()), /must be after/);
    assert.throws(() => commitIdentity('v0.0.58', overrides, parent(), later), /must be after/);
  }
});

test('override rejects invalid dates, dates before the window and its excluded upper bound', () => {
  for (const date of ['bad', '2026-10-08T23:59:59+08:00', '2026-10-09T09:00:00+08:00']) {
    assert.throws(() => commitIdentity('v0.0.58', { 'v0.0.58': { ...overrides['v0.0.58'], date } }, parent(), parent()));
  }
});

function fixture(t, { tag = 'v0.0.58', existing = '', published = false } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'zstock-release-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const directory of ['release-assets', 'updates', '.github', 'docs/releases']) fs.mkdirSync(path.join(root, directory), { recursive: true });
  fs.writeFileSync(path.join(root, '.github/release-commit-dates.json'), JSON.stringify(overrides));
  fs.writeFileSync(path.join(root, `docs/releases/${tag}.md`), 'Reviewed release notes');
  const packages = ASSET_NAMES.map((name) => {
    const bytes = Buffer.from(`fixture package ${name}`);
    fs.writeFileSync(path.join(root, 'release-assets', name), bytes);
    return { name, size: bytes.length, state: 'uploaded', digest: `sha256:${createHash('sha256').update(bytes).digest('hex')}` };
  });
  const manifest = { version: tag.slice(1), release_url: `https://github.com/csic21/zstock/releases/tag/${tag}`, platforms: {} };
  for (const platform of ['macos-arm64', 'macos-x64', 'windows-x64', 'linux-x64']) {
    const asset = packages.find((p) => p.name === `zstock-${platform}.zip`);
    manifest.platforms[platform] = { url: `https://github.com/csic21/zstock/releases/download/${tag}/${asset.name}`, sha256: asset.digest.slice(7) };
  }
  const content = JSON.stringify(manifest);
  fs.writeFileSync(path.join(root, 'updates/stable.json'), content);
  const calls = [];
  const assets = published ? [...packages] : [];
  const github = { rest: { git: {
    getRef: async ({ ref }) => ({ data: { object: { type: 'commit', sha: ref.startsWith('tags/') ? SOURCE : HEAD } } }),
    getCommit: async () => ({ data: parent() }),
    createTree: async (args) => { calls.push(['tree', args]); return { data: { sha: 'new-tree' } }; },
    createCommit: async (args) => { calls.push(['commit', args]); return { data: { sha: 'new-commit' } }; },
    updateRef: async (args) => { calls.push(['ref', args]); },
  }, repos: {
    getContent: async () => ({ data: { content: Buffer.from(existing === 'same' ? content : existing).toString('base64') } }),
    getReleaseByTag: async () => {
      if (published) return { data: { id: 1, draft: false } };
      throw Object.assign(new Error('not found'), { status: 404 });
    },
    createRelease: async (args) => { calls.push(['create-release', args]); return { data: { id: 1, draft: true } }; },
    listReleaseAssets: () => {},
    uploadReleaseAsset: async (args) => {
      calls.push(['upload', args.name]);
      const asset = packages.find((p) => p.name === args.name);
      assets.push(asset);
      return { data: asset };
    },
    updateRelease: async (args) => { calls.push(['publish', args]); },
  } }, paginate: async () => [...assets] };
  return { root, github, context: { repo: { owner: 'csic21', repo: 'zstock' } }, core: { info: () => {} },
    tag, sourceSha: SOURCE, publicationHead: HEAD, packages, manifest, content, calls, assets };
}

test('all nine packages are verified before publication, then manifest advances without force', async (t) => {
  const f = fixture(t);
  await publishRelease(f);
  assert.equal(f.calls.filter(([name]) => name === 'upload').length, 9);
  assert.equal(f.calls[0][0], 'create-release');
  assert.equal(f.calls[0][1].draft, true);
  assert.equal(f.calls[0][1].body, 'Reviewed release notes');
  assert.equal(f.calls[10][0], 'publish');
  const commit = f.calls.find(([name]) => name === 'commit')[1];
  assert.deepEqual(commit.parents, [HEAD]);
  assert.equal(commit.author.date, '2026-10-09T08:59:00+08:00');
  assert.equal(commit.committer.date, commit.author.date);
  assert.equal(f.calls.at(-1)[1].force, false);
  assert.equal(f.calls.find(([name]) => name === 'tree')[1].tree[0].path, 'updates/stable.json');
});

test('future releases do not inherit a historical author or committer', async (t) => {
  const f = fixture(t, { tag: 'v0.0.59' });
  await publishRelease(f);
  const commit = f.calls.find(([name]) => name === 'commit')[1];
  assert.equal(Object.hasOwn(commit, 'author'), false);
  assert.equal(Object.hasOwn(commit, 'committer'), false);
});

test('manifest validates version, package URL and exact real hash', (t) => {
  const f = fixture(t);
  validateManifest(f.content, f.tag, 'csic21/zstock', f.packages);
  for (const change of [
    (m) => { m.version = '0.0.57'; },
    (m) => { m.platforms['linux-x64'].sha256 = '0'.repeat(64); },
    (m) => { m.platforms['windows-x64'].url = 'https://invalid.test/package'; },
    (m) => { delete m.platforms['macos-arm64']; },
  ]) {
    const value = JSON.parse(f.content); change(value);
    assert.throws(() => validateManifest(JSON.stringify(value), f.tag, 'csic21/zstock', f.packages));
  }
});

test('missing local platform prevents all remote mutations', async (t) => {
  const f = fixture(t);
  fs.unlinkSync(path.join(f.root, 'release-assets', 'zstock-linux-x64.zip'));
  await assert.rejects(publishRelease(f), /ENOENT/);
  assert.equal(f.calls.length, 0);
});

test('bad parent date prevents release and manifest mutations', async (t) => {
  const f = fixture(t);
  f.github.rest.git.getCommit = async () => ({ data: { ...parent(), committer: { date: '2026-10-09T02:00:00Z' } } });
  await assert.rejects(publishRelease(f), /must be after/);
  assert.equal(f.calls.length, 0);
});

test('advanced main prevents release mutation', async (t) => {
  const f = fixture(t);
  f.github.rest.git.getRef = async ({ ref }) => ({ data: { object: { type: 'commit', sha: ref.startsWith('tags/') ? SOURCE : 'c'.repeat(40) } } });
  await assert.rejects(publishRelease(f), /main advanced/);
  assert.equal(f.calls.length, 0);
});

test('main changing during upload leaves release private', async (t) => {
  const f = fixture(t); let reads = 0;
  f.github.rest.git.getRef = async ({ ref }) => ({ data: { object: { type: 'commit', sha: ref.startsWith('tags/') ? SOURCE : (++reads === 1 ? HEAD : 'c'.repeat(40)) } } });
  await assert.rejects(publishRelease(f), /remains a draft/);
  assert.equal(f.calls.some(([name]) => ['publish', 'commit', 'ref'].includes(name)), false);
});

test('uploaded checksum mismatch never publishes', async (t) => {
  const f = fixture(t);
  f.github.rest.repos.uploadReleaseAsset = async () => ({ data: { ...f.packages[0], digest: 'sha256:bad' } });
  await assert.rejects(publishRelease(f), /SHA-256 mismatch/);
  assert.equal(f.calls.some(([name]) => name === 'publish'), false);
});

test('asset state and missing server digest fail closed', () => {
  const expected = { name: 'package.zip', size: 42, digest: 'sha256:abc' };
  assert.throws(() => verifyAsset({ ...expected, state: 'new' }, expected));
  assert.throws(() => verifyAsset({ ...expected, state: 'uploaded', digest: null }, expected));
});

test('matching published release and manifest are idempotent', async (t) => {
  const f = fixture(t, { existing: 'same', published: true });
  f.github.rest.git.getRef = async ({ ref }) => ({ data: { object: { type: 'commit', sha: ref.startsWith('tags/') ? SOURCE : 'c'.repeat(40) } } });
  await publishRelease(f);
  assert.equal(f.calls.length, 0);
});

test('unexpected assets prevent public release', async (t) => {
  const f = fixture(t);
  f.assets.push({ name: 'unexpected.txt' });
  await assert.rejects(publishRelease(f), /exactly the nine/);
  assert.equal(f.calls.some(([name]) => name === 'publish'), false);
});

test('concurrent main write cannot be forced over', async (t) => {
  const f = fixture(t);
  f.github.rest.git.updateRef = async (args) => {
    assert.equal(args.force, false);
    throw Object.assign(new Error('Not a fast forward'), { status: 422 });
  };
  await assert.rejects(publishRelease(f), /Not a fast forward/);
});

test('real manifest generator produces hashes and URLs accepted by publisher', (t) => {
  const f = fixture(t);
  fs.mkdirSync(path.join(f.root, 'scripts'));
  const script = path.join(f.root, 'scripts/update-manifest.sh');
  fs.copyFileSync(path.join(__dirname, 'update-manifest.sh'), script);
  const hashes = ['macos-arm64', 'macos-x64', 'windows-x64', 'linux-x64']
    .map((platform) => f.packages.find((asset) => asset.name === `zstock-${platform}.zip`).digest.slice(7));
  const result = require('node:child_process').spawnSync('bash', [script, f.tag, ...hashes], {
    env: { ...process.env, GITHUB_REPOSITORY: 'csic21/zstock', ASSET_PREFIX: 'zstock' }, encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  validateManifest(fs.readFileSync(path.join(f.root, 'updates/stable.json'), 'utf8'), f.tag, 'csic21/zstock', f.packages);
});

test('tag retargeted after resolution prevents all publication mutations', async (t) => {
  const f = fixture(t);
  f.github.rest.git.getRef = async () => ({ data: { object: { type: 'commit', sha: HEAD } } });
  await assert.rejects(publishRelease(f), /Release tag changed/);
  assert.equal(f.calls.length, 0);
});

for (const scenario of [
  { name: 'existing draft prerelease', draft: true, existing: '' },
  { name: 'existing published prerelease', draft: false, existing: '' },
  { name: 'published prerelease with matching stable manifest', draft: false, existing: 'same' },
]) {
  test(`${scenario.name} fails closed before any release or manifest mutation`, async (t) => {
    const f = fixture(t, { published: true, existing: scenario.existing });
    f.github.rest.repos.getReleaseByTag = async () => ({ data: { id: 1, draft: scenario.draft, prerelease: true } });
    await assert.rejects(publishRelease(f), /Existing release is a prerelease/);
    assert.equal(f.calls.length, 0);
  });
}
