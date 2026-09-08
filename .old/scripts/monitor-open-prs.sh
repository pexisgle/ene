#!/usr/bin/env bash
set -euo pipefail

# Report open PRs whose checks failed or are still running, or which conflict
# with their base branch. Pending runs appear so the caller can re-run this
# script after they finish instead of mistaking them for a clean state.

repo_root=$(git rev-parse --show-toplevel)
report="$repo_root/.monitor-open-prs.json"

# Keep the scratch file out of git status even in linked worktrees, whose
# .git path is a file pointing into the main repository's worktree metadata.
exclude=$(git rev-parse --path-format=absolute --git-path info/exclude)
if ! grep -qxF ".monitor-open-prs.json" "$exclude" 2>/dev/null; then
  mkdir -p "$(dirname "$exclude")"
  echo ".monitor-open-prs.json" >>"$exclude"
fi

gh pr list \
  --state open \
  --limit 200 \
  --json number,isDraft,mergeable,mergeStateStatus,statusCheckRollup,url,title,headRefName,baseRefName \
  >"$report"

jq -c '
  .[]
  | {
      pr: .number,
      draft: .isDraft,
      title: .title,
      head: .headRefName,
      base: .baseRefName,
      mergeable: .mergeable,
      mergeStateStatus: .mergeStateStatus,
      url: .url,
      pending_checks:
        ([.statusCheckRollup // [] | .[] |
          select(.status == "IN_PROGRESS"
            or .status == "QUEUED"
            or .status == "PENDING")])
        | map(.name // .workflowName // .detailsUrl),
      failed_checks:
        ([.statusCheckRollup // [] | .[] |
          select(.conclusion == "FAILURE")])
        | map(.name // .workflowName // .detailsUrl)
    }
  | select(
      .mergeable == "CONFLICTING"
      or .mergeStateStatus == "DIRTY"
      or (.failed_checks | length > 0)
      or (.pending_checks | length > 0)
    )
' "$report"
