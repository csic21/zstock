'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { parseReleaseRequest, resolveRelease } = require('./resolve-release.cjs');
const SOURCE = 'a'.repeat(40);
const HEAD = 'b'.repeat(40);
const request = { schema_version: 1, tag: 'v0.0.58', source_sha: SOURCE };
const requestCommit = () => ({ parents: [{ sha: SOURCE }], files: [{ filename: '.github/release-request.json', status: 'added' }] });

function fixture(overrides = {}) {
  const calls = [];
  const github = { rest: { repos: {
    getCommit: async () => ({ data: requestCommit() }),
    getContent: async ({ path }) => ({ data: { content: Buffer.from(path === 'Cargo.toml'
      ? '[package]\nname = "stock"\nversion = "0.0.58"\n' : JSON.stringify(request)).toString('base64') } }),
  }, git: {
    getRef: async ({ ref }) => {
      if (ref === 'heads/main') return { data: { object: { sha: HEAD } } };
      throw Object.assign(new Error('missing'), { status: 404 });
    },
    createRef: async (args) => calls.push(args),
  } } };
  Object.assign(github.rest.git, overrides);
  const output = {};
  return { github, calls, output, core: { setOutput: (key, value) => { output[key] = value; } },
    context: { repo: { owner: 'csic21', repo: 'zstock' }, eventName: 'push', ref: 'refs/heads/main', sha: HEAD } };
}

test('request must be a request-only direct child of source', () => {
  assert.deepEqual(parseReleaseRequest(JSON.stringify(request), requestCommit()), request);
  for (const commit of [
    { ...requestCommit(), parents: [{ sha: HEAD }] },
    { ...requestCommit(), parents: [{ sha: SOURCE }, { sha: HEAD }] },
    { ...requestCommit(), files: [...requestCommit().files, { filename: 'Cargo.toml', status: 'modified' }] },
    { ...requestCommit(), files: [{ filename: '.github/release-request.json', status: 'removed' }] },
  ]) assert.throws(() => parseReleaseRequest(JSON.stringify(request), commit), /only parent/);
});

test('malformed request or tag traversal is rejected', () => {
  for (const invalid of [{ ...request, schema_version: 2 }, { ...request, source_sha: 'main' },
    { ...request, tag: '../main' }, { ...request, tag: 'v0.0.58-beta' }]) {
    assert.throws(() => parseReleaseRequest(JSON.stringify(invalid), requestCommit()), /Invalid release request/);
  }
});

test('request creates exact source tag and resolves source for same-run builds', async () => {
  const f = fixture();
  const result = await resolveRelease(f);
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].sha, SOURCE);
  assert.equal(f.calls[0].ref, 'refs/tags/v0.0.58');
  assert.deepEqual(result, { sourceSha: SOURCE, publicationHead: HEAD, tag: 'v0.0.58', publish: true });
  assert.equal(f.output.publish, 'true');
});

test('manual runs only build artifacts and never create a tag', async () => {
  const f = fixture();
  f.context.eventName = 'workflow_dispatch';
  f.context.ref = 'refs/tags/v0.0.58';
  const result = await resolveRelease(f);
  assert.equal(result.publish, false);
  assert.equal(f.calls.length, 0);
});

test('existing matching tag is idempotent', async () => {
  const f = fixture({ getRef: async ({ ref }) => ({ data: { object: {
    type: 'commit', sha: ref === 'heads/main' ? HEAD : SOURCE,
  } } }) });
  await resolveRelease(f);
  assert.equal(f.calls.length, 0);
});

test('existing wrong tag is never overwritten', async () => {
  const f = fixture({ getRef: async () => ({ data: { object: { type: 'commit', sha: HEAD } } }) });
  await assert.rejects(resolveRelease(f), /will not be overwritten/);
  assert.equal(f.calls.length, 0);
});

test('main advancement fails before tag creation', async () => {
  const f = fixture({ getRef: async () => ({ data: { object: { sha: 'c'.repeat(40) } } }) });
  await assert.rejects(resolveRelease(f), /main advanced/);
  assert.equal(f.calls.length, 0);
});

test('Cargo version mismatch fails before tag creation', async () => {
  const f = fixture();
  const read = f.github.rest.repos.getContent;
  f.github.rest.repos.getContent = async (args) => args.path === 'Cargo.toml'
    ? { data: { content: Buffer.from('version = "0.0.57"').toString('base64') } } : read(args);
  await assert.rejects(resolveRelease(f), /does not match/);
  assert.equal(f.calls.length, 0);
});

test('ordinary tag push keeps its exact source', async () => {
  const f = fixture({ getRef: async () => ({ data: { object: { type: 'commit', sha: SOURCE } } }) });
  f.context.sha = SOURCE;
  f.context.ref = 'refs/tags/v0.0.58';
  const result = await resolveRelease(f);
  assert.equal(result.publicationHead, SOURCE);
  assert.equal(result.sourceSha, SOURCE);
  assert.equal(result.publish, true);
});
