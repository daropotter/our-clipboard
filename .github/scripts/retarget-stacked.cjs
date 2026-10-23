// Runs only trusted repository code; PR content is never checked out or executed.
module.exports = async function retargetStacked({ github, context, core }) {
  const merged = context.payload.pull_request;
  const { owner, repo } = context.repo;
  const repository = `${owner}/${repo}`;
  if (!merged?.merged || merged.head.repo?.full_name !== repository) return;

  const previousBase = merged.head.ref;
  const nextBase = merged.base.ref;
  if (previousBase === nextBase) return;

  // Collect all pages before editing: changing bases changes list membership.
  const candidates = await github.paginate(github.rest.pulls.list, {
    owner, repo, state: 'open', base: previousBase, per_page: 100,
  });
  const failures = [];
  for (const candidate of candidates) {
    if (candidate.number === merged.number) continue;
    try {
      // GitHub or a maintainer may already have retargeted/closed this PR.
      const { data: current } = await github.rest.pulls.get({
        owner, repo, pull_number: candidate.number,
      });
      if (current.state !== 'open' || current.base.ref !== previousBase) continue;
      await github.rest.pulls.update({
        owner, repo, pull_number: candidate.number, base: nextBase,
      });
      core.info(`Retargeted PR #${candidate.number} to ${nextBase}.`);
    } catch (error) {
      failures.push(candidate.number);
      core.error(`Could not retarget PR #${candidate.number} (HTTP ${error.status ?? 'unknown'}).`);
    }
  }
  if (failures.length) throw new Error(`Failed to retarget PRs: ${failures.join(', ')}`);
};
