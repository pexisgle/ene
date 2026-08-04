# Git Tool Guide

`ene-plugin-git` provides read-only git repository inspection tools: working
tree status, diffs, commit history, branch lists, remote configuration, and
per-line blame. It is a built-in tool plugin and starts automatically on fresh
installs.

All actions are strictly read-only — there is no commit, push, pull, fetch,
or clone. Every action returns structured JSON.

## Actions

| Action | Purpose |
|---|---|
| `git.status` | Current branch (or detached HEAD) plus per-file staged/unstaged/untracked/conflicted state |
| `git.diff` | Unified diff text and/or stat summary of unstaged or staged changes |
| `git.log` | Commit history with authors, timestamps, and parents |
| `git.branch` | Local (optionally remote-tracking) branches with upstream and ahead/behind counts |
| `git.remote` | Configured remotes with fetch and push URLs |
| `git.blame` | Per-line attribution of a committed file |

## Repository path and workspace sandbox

Every action takes an optional `path` argument defaulting to the current
directory. The path is resolved and canonicalized, and it must lie inside one
of the workspace's allowed directories — the same sandbox contract the
filesystem plugin uses. The repository discovered from that path must also
have its working tree inside the allowed workspace, so a path inside the
workspace can never expose the history of an ancestor repository living
outside it.

Repository-relative file arguments (`git.diff` `pathFilter`, `git.blame`
`file`) are validated: relative paths only, no `..` segments, no absolute
paths.

## Examples

Status of the current repository:

```json
{"path": "."}
```

Staged diff of a repository at a specific path, as a stat summary:

```json
{"path": "/home/me/project", "staged": true, "format": "both"}
```

Last ten commits:

```json
{"path": ".", "maxCount": 10}
```

Blame lines 1-5 of `src/main.rs`:

```json
{"path": ".", "file": "src/main.rs", "startLine": 1, "endLine": 5}
```

## Notes

- `git.diff` compares the working tree against the index by default
  (`staged: false`) and the index against HEAD with `staged: true`; an empty
  diff returns zero-change output rather than an error.
- `git.log` walks from HEAD by default; pass `branch` to walk from another
  branch or ref. `maxCount` is capped at 100.
- `git.blame` attributes lines of the committed HEAD version; uncommitted
  working-tree edits are not reflected.
- Timestamps are RFC 3339 strings preserving the commit's original timezone
  offset.
- No configuration is required. The plugin is enabled by default
  (`tools.list.git.enable = true` in `settings.json`) and can be disabled like
  any other tool plugin.
