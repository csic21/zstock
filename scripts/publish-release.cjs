'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { createHash } = require('node:crypto');

const ASSET_NAMES = [
  'zstock-macos-arm64.zip', 'zstock-macos-arm64.dmg', 'zstock-macos-arm64.pkg',
  'zstock-macos-x64.zip', 'zstock-macos-x64.dmg', 'zstock-macos-x64.pkg',
  'zstock-windows-x64.zip', 'zstock-windows-x64-setup.exe', 'zstock-linux-x64.zip',
];
const MANIFEST_PATH = 'updates/stable.json';

function commitIdentity(tag, overrides, source, parent) {
  const override = overrides[tag];
  if (!override) return {}; // Every other release retains GitHub's real commit time.
  const timestamp = (value) => {
    if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})$/.test(value)
        || !Number.isFinite(Date.parse(value))) throw new Error('Invalid release commit timestamp');
    return Date.parse(value);
  };
  const date = timestamp(override.date);
  const start = timestamp(override.window_start);
  const end = timestamp(override.window_end_exclusive);
  if (start >= end || end - start > 24 * 60 * 60 * 1000 || date < start || date >= end) {
    throw new Error('Release commit timestamp is outside its bounded window');
  }
  for (const commit of [source, parent]) {
    if (date <= timestamp(commit.author?.date) || date <= timestamp(commit.committer?.date)) {
      throw new Error('Release commit timestamp must be after source and parent author/committer dates');
    }
  }
  const identity = { name: 'github-actions[bot]', email: '41898282+github-actions[bot]@users.noreply.github.com', date: override.date };
  return { author: { ...identity }, committer: { ...identity } };
}

function readPackages(root) {
  return ASSET_NAMES.map((name) => {
    const filename = path.join(root, 'release-assets', name);
    const content = fs.readFileSync(filename);
    if (!content.length) throw new Error(`Empty release package: ${name}`);
    return { name, filename, size: content.length, digest: `sha256:${createHash('sha256').update(content).digest('hex')}` };
  });
}

function validateManifest(content, tag, repository, packages) {
  const manifest = JSON.parse(content);
  const releaseUrl = `https://github.com/${repository}/releases`;
  if (manifest.version !== tag.slice(1) || manifest.release_url !== `${releaseUrl}/tag/${tag}`) {
    throw new Error('Generated stable manifest has the wrong version or release URL');
  }
  const platforms = ['macos-arm64', 'macos-x64', 'windows-x64', 'linux-x64'];
  if (Object.keys(manifest.platforms || {}).sort().join(',') !== [...platforms].sort().join(',')) {
    throw new Error('Generated stable manifest does not contain exactly four platforms');
  }
  for (const platform of platforms) {
    const name = `zstock-${platform}.zip`;
    const expected = packages.find((asset) => asset.name === name);
    const entry = manifest.platforms[platform];
    if (entry?.url !== `${releaseUrl}/download/${tag}/${name}` || `sha256:${entry.sha256}` !== expected?.digest) {
      throw new Error(`Generated stable manifest does not match built package: ${platform}`);
    }
  }
}

function verifyAsset(actual, expected) {
  if (actual.name !== expected.name || actual.state !== 'uploaded'
      || actual.size !== expected.size || actual.digest !== expected.digest) {
    throw new Error(`Release asset size/state/SHA-256 mismatch: ${expected.name}`);
  }
}

async function publishRelease({ github, context, core, tag, sourceSha, publicationHead, root = process.cwd() }) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag)) throw new Error('Invalid stable release tag');
  if (!/^[a-f0-9]{40}$/.test(sourceSha || '') || !/^[a-f0-9]{40}$/.test(publicationHead || '')) {
    throw new Error('Missing resolved release source or publication head');
  }
  const repo = context.repo;
  const packages = readPackages(root);
  const content = fs.readFileSync(path.join(root, MANIFEST_PATH), 'utf8');
  validateManifest(content, tag, `${repo.owner}/${repo.repo}`, packages);
  let target = (await github.rest.git.getRef({ ...repo, ref: `tags/${tag}` })).data.object;
  for (let depth = 0; target.type === 'tag' && depth < 5; depth++) {
    target = (await github.rest.git.getTag({ ...repo, tag_sha: target.sha })).data.object;
  }
  if (target.type !== 'commit' || target.sha !== sourceSha) {
    throw new Error('Release tag changed; refusing to publish packages for a different source');
  }
  const { data: main } = await github.rest.git.getRef({ ...repo, ref: 'heads/main' });
  let existingContent = '';
  try {
    const { data: file } = await github.rest.repos.getContent({ ...repo, path: MANIFEST_PATH, ref: main.object.sha });
    existingContent = Buffer.from(file.content, 'base64').toString('utf8');
  } catch (error) {
    if (error.status !== 404) throw error;
  }
  const unchanged = existingContent === content;
  if (!unchanged && main.object.sha !== publicationHead) {
    throw new Error('main advanced; refusing to publish or overwrite its stable manifest');
  }
  let parent;
  let identity = {};
  if (!unchanged) {
    parent = (await github.rest.git.getCommit({ ...repo, commit_sha: publicationHead })).data;
    const source = sourceSha === publicationHead ? parent
      : (await github.rest.git.getCommit({ ...repo, commit_sha: sourceSha })).data;
    if (sourceSha !== publicationHead && (parent.parents?.length !== 1 || parent.parents[0].sha !== sourceSha)) {
      throw new Error('Publication head is not a direct child of the release source');
    }
    const datesPath = path.join(root, '.github/release-commit-dates.json');
    const overrides = fs.existsSync(datesPath) ? JSON.parse(fs.readFileSync(datesPath, 'utf8')) : {};
    identity = commitIdentity(tag, overrides, source, parent);
  }
  let release;
  try {
    release = (await github.rest.repos.getReleaseByTag({ ...repo, tag })).data;
  } catch (error) {
    if (error.status !== 404) throw error;
  }
  if (release?.prerelease) {
    throw new Error('Existing release is a prerelease; refusing stable publication or manifest advancement');
  }
  if (!release) {
    const notesPath = path.join(root, 'docs/releases', `${tag}.md`);
    const body = fs.existsSync(notesPath) ? fs.readFileSync(notesPath, 'utf8')
      : (await github.rest.repos.generateReleaseNotes({ ...repo, tag_name: tag, target_commitish: sourceSha })).data.body;
    release = (await github.rest.repos.createRelease({ ...repo, tag_name: tag, target_commitish: sourceSha,
      name: tag, body, draft: true, prerelease: false })).data;
  }
  const listAssets = () => github.paginate(github.rest.repos.listReleaseAssets, { ...repo, release_id: release.id });
  const assets = await listAssets();
  for (const expected of packages) {
    const actual = assets.find((asset) => asset.name === expected.name);
    if (actual) {
      verifyAsset(actual, expected);
    } else {
      if (!release.draft) throw new Error(`Published release is missing ${expected.name}; refusing to mutate it`);
      const uploaded = await github.rest.repos.uploadReleaseAsset({ ...repo, release_id: release.id,
        name: expected.name, data: fs.readFileSync(expected.filename),
        headers: { 'content-type': 'application/octet-stream', 'content-length': expected.size } });
      verifyAsset(uploaded.data, expected);
    }
  }
  const uploaded = await listAssets();
  if (uploaded.length !== packages.length) throw new Error('Release must contain exactly the nine expected platform packages');
  for (const expected of packages) {
    const actual = uploaded.find((asset) => asset.name === expected.name);
    if (!actual) throw new Error(`Release is missing ${expected.name}`);
    verifyAsset(actual, expected);
  }
  // Check again after uploads. If main moved, the complete release remains a draft.
  if (!unchanged && (await github.rest.git.getRef({ ...repo, ref: 'heads/main' })).data.object.sha !== publicationHead) {
    throw new Error('main advanced during upload; release remains a draft');
  }
  if (release.draft) {
    await github.rest.repos.updateRelease({ ...repo, release_id: release.id, draft: false, make_latest: 'true' });
  }
  if (unchanged) {
    core.info('Verified release and stable manifest are already current');
    return;
  }
  const { data: tree } = await github.rest.git.createTree({ ...repo, base_tree: parent.tree.sha,
    tree: [{ path: MANIFEST_PATH, mode: '100644', type: 'blob', content }] });
  const { data: commit } = await github.rest.git.createCommit({ ...repo,
    message: `chore(update): publish stable manifest for ${tag}`, tree: tree.sha,
    parents: [publicationHead], ...identity });
  // Never force: if main changed since preflight, this sibling commit cannot replace it.
  await github.rest.git.updateRef({ ...repo, ref: 'heads/main', sha: commit.sha, force: false });
  core.info(`Published ${tag} and stable manifest ${commit.sha}`);
}

module.exports = { ASSET_NAMES, commitIdentity, validateManifest, verifyAsset, publishRelease };
