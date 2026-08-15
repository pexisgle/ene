use ene_plugin_proto::ToolError;

/// Returns the current branch shorthand and, when detached, the short `HEAD`
/// oid.
pub(crate) async fn head_info(workdir: &str) -> (Option<String>, Option<String>) {
    let broker = crate::broker::broker();
    // An unborn `HEAD` has no commit to resolve; report neither branch nor
    // detached oid, matching the pre-broker behavior.
    let Ok(verify) = broker
        .run_git(workdir, &["rev-parse", "--verify", "HEAD"])
        .await
    else {
        return (None, None);
    };
    if !verify.ok() {
        return (None, None);
    }
    let Ok(branch_run) = broker.run_git(workdir, &["branch", "--show-current"]).await else {
        return (None, None);
    };
    let branch = branch_run
        .stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    if let Some(branch) = branch {
        (Some(branch), None)
    } else {
        let Ok(detached) = broker
            .run_git(workdir, &["rev-parse", "--short", "HEAD"])
            .await
        else {
            return (None, None);
        };
        let oid = detached
            .stdout
            .lines()
            .next()
            .map(str::trim)
            .map(str::to_string);
        (None, oid)
    }
}

/// Maps a failed `git log` / `rev-list` run to `NoCommits` for an unborn
/// `HEAD`, otherwise to the generic run error.
pub(crate) fn no_commits_or(run: &crate::broker::GitRun) -> ToolError {
    let stderr = run.stderr.to_ascii_lowercase();
    if stderr.contains("does not have any commits")
        || stderr.contains("unknown revision")
        || stderr.contains("bad revision")
    {
        ToolError::from(crate::error::GitError::NoCommits)
    } else {
        crate::sandbox::git_run_error(run)
    }
}
