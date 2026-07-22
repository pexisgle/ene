# Release

How maintainers cut a Linux release of ene. The pipeline is tag-driven: pushing a `v*` tag builds binaries, packages artifacts, generates a changelog, and publishes a GitHub Release.

[← Developer guide](index.md) · [日本語](../ja/guide/release.md)

## Overview

| Step | What happens |
|------|----------------|
| Tag push (`v*`) | `.github/workflows/release.yml` runs on `ubuntu-latest` |
| Build | `nix develop --command cargo build --release` for `ene-cli`, `ene-desktop`, and all built-in tool binaries |
| Package | `scripts/package-linux-release.sh` produces a CLI tarball and a desktop `.deb` |
| Changelog | `git-cliff` reads Conventional Commits since the previous tag (`cliff.toml`) |
| Publish | `softprops/action-gh-release` uploads assets and sets the release body |

### Release artifacts (Linux x86_64)

| Asset | Contents |
|-------|----------|
| `ene-cli-<version>-linux-x86_64.tar.gz` | `ene-cli` plus `tools/` with built-in tool binaries |
| `ene-desktop_<version>_amd64.deb` | `ene-desktop` in `/usr/bin`, tools in `/usr/bin/tools/`, `.desktop` entry |

Windows and macOS installers are out of scope for now ([#244](https://github.com/pexisgle/ene/issues/244)).

## Prerequisites

- Write access to the repository (to push tags).
- Commits on `main` follow [Conventional Commits](https://www.conventionalcommits.org/) so `git-cliff` can group the changelog (`feat:`, `fix:`, `docs:`, …).
- Workspace version in the root `Cargo.toml` `[workspace.package]` section should match the tag (without the `v` prefix).

## Cutting a release

1. **Bump the workspace version** (when needed):

   ```toml
   # Cargo.toml
   [workspace.package]
   version = "0.2.0"
   ```

2. **Merge release prep to `main`** (version bump, any last-minute fixes, updated docs).

3. **Create and push an annotated tag**:

   ```bash
   git tag -a v0.2.0 -m "ene v0.2.0"
   git push origin v0.2.0
   ```

4. **Watch CI** — the [Release workflow](https://github.com/pexisgle/ene/actions/workflows/release.yml) builds, packages, and opens the GitHub Release.

5. **Verify the release page** — changelog sections, attached `.tar.gz` and `.deb`, and install smoke tests on a Linux machine.

## Installing release builds

### CLI (tarball)

```bash
tar -xzf ene-cli-0.2.0-linux-x86_64.tar.gz
cd ene-cli-0.2.0-linux-x86_64
./ene-cli --help
```

Keep the `tools/` directory next to `ene-cli` (release builds look for built-in tools in `<exe_dir>/tools/`).

### Desktop (.deb)

```bash
sudo dpkg -i ene-desktop_0.2.0_amd64.deb
# install missing runtime libraries if dpkg reports dependency errors:
sudo apt-get install -f
ene-desktop
```

The package declares common GTK/Wayland/PipeWire dependencies; some GPU or portal libraries may still be required on minimal systems (see [Desktop app](apps/desktop.md)).

## Local dry run

Reproduce packaging without publishing:

```bash
# Build release binaries
nix develop --command cargo build --release \
  -p ene-cli -p ene-desktop \
  -p ene-tool-fs -p ene-tool-web -p ene-tool-utility \
  -p ene-tool-app -p ene-tool-browser

# Package (version string is arbitrary for local testing)
bash scripts/package-linux-release.sh 0.2.0

# Preview changelog for the latest tag
nix develop --command git cliff --latest --strip header
```

Artifacts land in `dist/`.

## Changelog configuration

- Config: `cliff.toml` at the repository root.
- `git-cliff` is available in the Nix dev shell (`flake.nix`).
- Release notes use only the latest tag section (`git cliff --latest --strip header`).
- `chore(release):` commits are excluded from the generated body.

To preview the full changelog locally:

```bash
nix develop --command git cliff --config cliff.toml
```

## Crate metadata

All workspace crates set `publish = false` and inherit `license`, `repository`, and `version` from `[workspace.package]`. Application crates (`ene-cli`, `ene-desktop`) use `version.workspace = true` so the workspace version is the single source of truth.

## Troubleshooting

| Symptom | Likely cause |
|---------|----------------|
| Empty or sparse changelog | Commits are not Conventional Commits, or `fetch-depth: 0` was removed from the workflow checkout |
| `dpkg-deb` failure in CI | Packaging script error; run `bash scripts/package-linux-release.sh <ver>` locally |
| Desktop starts but tools missing | Built-in tools must live in `tools/` next to the executable (tarball layout) or `/usr/bin/tools/` (`.deb` layout) |
| Workflow does not run | Tag must match `v*` (e.g. `v0.1.0`, not `0.1.0`) |

## Related

- [Getting started](getting-started.md) — day-to-day development builds
- [CI](../../.github/workflows/ci.yml) — format, clippy, and tests on `main`
- Issue [#244](https://github.com/pexisgle/ene/issues/244) — release automation tracking
