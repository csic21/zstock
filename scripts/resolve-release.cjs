'use strict';

const REQUEST_PATH = '.github/release-request.json';
const STABLE_TAG = /^v\d+\.\d+\.\d+$/;

function parseReleaseRequest(content, commit) {
  const request = JSON.parse(content);
  if (request.schema_version !== 1 || !STABLE_TAG.test(request.tag)
      || !/^[a-f0-9]{40}$/.test(request.source_sha || '')) {
    throw new Error('Invalid release request schema, stable tag, or source SHA');
  }
  if (commit.parents?.length !== 1 || commit.parents[0].sha !== request.source_sha
      || commit.files?.length !== 1 || commit.files[0].filename !== REQUEST_PATH
      || !['added', 'modified'].includes(commit.files[0].status)) {
    throw new Error('Release request must change only its request file, with source_sha as its only parent');
  }
  return request;
}

async function resolveRelease({ github, context, core }) {
  const repo = context.repo;
  let sourceSha = context.sha;
  let tag = '';
  let publish = false;
  if (context.eventName === 'push' && context.ref === 'refs/heads/main') {
    const { data: commit } = await github.rest.repos.getCommit({ ...repo, ref: context.sha });
    const { data: file } = await github.rest.repos.getContent({ ...repo, path: REQUEST_PATH, ref: context.sha });
    const request = parseReleaseRequest(Buffer.from(file.content, 'base64').toString('utf8'), commit);
    sourceSha = request.source_sha;
    tag = request.tag;
    publish = true;
  } else if (context.eventName === 'push' && context.ref.startsWith('refs/tags/')) {
    tag = context.ref.slice('refs/tags/'.length);
    publish = true;
  }
  if (publish) {
    if (!STABLE_TAG.test(tag)) throw new Error('Stable publication requires a vX.Y.Z tag');
    const { data: manifest } = await github.rest.repos.getContent({ ...repo, path: 'Cargo.toml', ref: sourceSha });
    const cargo = Buffer.from(manifest.content, 'base64').toString('utf8');
    const version = cargo.match(/^version = "([^"]+)"/m)?.[1];
    if (`v${version}` !== tag) throw new Error(`Tag ${tag} does not match source Cargo.toml ${version}`);
    const { data: main } = await github.rest.git.getRef({ ...repo, ref: 'heads/main' });
    if (main.object.sha !== context.sha) throw new Error('main advanced; refusing a stale release request/tag');
    let existing;
    try {
      existing = (await github.rest.git.getRef({ ...repo, ref: `tags/${tag}` })).data.object;
    } catch (error) {
      if (error.status !== 404) throw error;
    }
    if (existing) {
      // Existing annotated tags are supported, but cannot be retargeted.
      for (let depth = 0; existing.type === 'tag' && depth < 5; depth++) {
        existing = (await github.rest.git.getTag({ ...repo, tag_sha: existing.sha })).data.object;
      }
      if (existing.type !== 'commit' || existing.sha !== sourceSha) {
        throw new Error(`Existing ${tag} points to a different source; it will not be overwritten`);
      }
    } else {
      await github.rest.git.createRef({ ...repo, ref: `refs/tags/${tag}`, sha: sourceSha });
    }
  }
  core.setOutput('source_sha', sourceSha);
  core.setOutput('publication_head', context.sha);
  core.setOutput('tag', tag);
  core.setOutput('publish', String(publish));
  return { sourceSha, publicationHead: context.sha, tag, publish };
}

module.exports = { parseReleaseRequest, resolveRelease };
