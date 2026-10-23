const { test } = require('node:test');
const assert = require('node:assert/strict');
const retarget = require('./retarget-stacked.cjs');

function fixture() {
  const updates = [];
  const listed = [];
  const candidates = [{ number: 2 }, { number: 3 }];
  const current = new Map(candidates.map(({ number }) => [number, {
    state: 'open', base: { ref: 'feature/parent' },
  }]));
  const context = {
    repo: { owner: 'daropotter', repo: 'our-clipboard' },
    payload: { pull_request: {
      number: 1, merged: true,
      head: { ref: 'feature/parent', repo: { full_name: 'daropotter/our-clipboard' } },
      base: { ref: 'release/next' },
    } },
  };
  const github = {
    paginate: async (endpoint, options) => {
      assert.equal(endpoint, github.rest.pulls.list);
      listed.push(options);
      return candidates;
    },
    rest: { pulls: {
      list: () => {},
      get: async ({ pull_number }) => ({ data: current.get(pull_number) }),
      update: async (options) => {
        updates.push(options);
        current.get(options.pull_number).base.ref = options.base;
      },
    } },
  };
  return { github, context, core: { info() {}, error() {} }, updates, listed, current, candidates };
}

test('retargets every direct child to the actual merge target, not main', async () => {
  const f = fixture();
  await retarget(f);
  assert.deepEqual(f.listed, [{ owner: 'daropotter', repo: 'our-clipboard', state: 'open', base: 'feature/parent', per_page: 100 }]);
  assert.deepEqual(f.updates.map(({ pull_number, base }) => [pull_number, base]), [[2, 'release/next'], [3, 'release/next']]);
});

test('ignores closed without merge and same-named heads from forks', async () => {
  for (const change of [
    f => { f.context.payload.pull_request.merged = false; },
    f => { f.context.payload.pull_request.head.repo.full_name = 'fork/our-clipboard'; },
    f => { f.context.payload.pull_request.head.repo = null; },
  ]) {
    const f = fixture(); change(f);
    await retarget(f);
    assert.equal(f.listed.length, 0);
    assert.equal(f.updates.length, 0);
  }
});

test('skips PRs already retargeted by GitHub or closed by a maintainer', async () => {
  const f = fixture();
  f.current.get(2).base.ref = 'another-target';
  f.current.get(3).state = 'closed';
  await retarget(f);
  assert.equal(f.updates.length, 0);
});

test('reruns do not update already retargeted PRs', async () => {
  const f = fixture();
  await retarget(f);
  await retarget(f);
  assert.equal(f.updates.length, 2);
});

test('continues after one API failure, then fails the job for visibility', async () => {
  const f = fixture();
  const update = f.github.rest.pulls.update;
  f.github.rest.pulls.update = async options => {
    if (options.pull_number === 2) throw Object.assign(new Error('denied'), { status: 403 });
    return update(options);
  };
  await assert.rejects(retarget(f), /Failed to retarget PRs: 2/);
  assert.deepEqual(f.updates.map(p => p.pull_number), [3]);
});
