# Sandbox, Broker, and Approvals

This page describes how Ene keeps plugins, sidecars, stdio MCP servers, and
downloaded code from touching things they were not allowed to touch. It
covers the three layers that replaced ad-hoc per-plugin permissions:

1. **OS sandbox** — the process sees only what the host granted.
2. **Broker channel** — all OS access is mediated by the host.
3. **Approval model** — what a plugin may request (manifest layer) and what
   is actually granted (policy layer).

## Principles

- Plugins, sidecars, stdio MCP servers, and the browser tool all live under
  the same artifact, broker, and resource management.
- A plugin never receives a socket, DNS access, an HTTP client, a cookie
  store, or an API key directly. It asks the host through the broker.
- User files are read and written through the `File` broker; plugins never
  get raw path grants.
- Automatic approval only skips the confirmation dialog. Signature, hash,
  size, connection-target, sandbox, and resource-limit checks are never
  disabled.
- There is no backward compatibility: everything migrates to the new
  manifest, IPC, and broker at once.

## OS sandbox

On Linux, an enabled plugin is started with:

- **Landlock** — a filesystem allowlist: read/exec only for the binary,
  system libraries, CA roots, assets, and artifacts; write only for the
  per-plugin temp dir, IPC sockets, and write-granted folders.
- **`no_new_privs` + seccomp** — privilege escalation and dangerous
  syscalls (`mount`, `ptrace`, `bpf`, `keyctl`, `setns`, `unshare`,
  `open_by_handle_at`, io_uring, …) are blocked.
- **rlimits** — file descriptors, address space, file size, CPU time, core
  dumps.
- **cgroup v2** (opt-in, needs a delegated cgroupfs) — memory, pids, and
  CPU shares for the process tree.
- **Network namespace** (opt-in, needs privileges) — the plugin has no
  direct network at all; everything goes through the `Network` broker.

On Windows, an enabled plugin runs in a **Job Object** (kill-on-close,
active-process, memory, and CPU-time limits) spawned suspended and attached
before resumption. AppContainer/low-integrity hardening is the documented
next step.

Every layer is fail-closed: if a required layer cannot be initialized, the
plugin does not start. Layers that need privileges (cgroup, network
namespace) are off by default; enable them explicitly when you run a host
that can provide them.

Per-plugin sandbox settings live at `plugins.list.<name>.sandbox`; the
global default is `plugins.sandbox`. Pure computation built-ins (`calc`,
`counter`, `random`) ship **sandboxed by default**, as do the
broker-migrated built-ins `web`, `fs`, and `openai`: `web` talks through
the `Network` broker (SSRF and redirect handling moved host-side), `fs`
routes all user-file I/O and shell execution through the `File` /
`Process` brokers — its tools keep absolute-path arguments, which the host
resolves against the configured grants (`plugins.list.<name>.fs_grants`)
by canonical containment — and `openai` mediates every API request through
the `Network` broker with the API key injected host-side by name (see
[Credential injection](#credential-injection)). Remaining built-ins
default to disabled until their migration to the broker channel lands.
Enabled plugins refuse to start when the kernel cannot enforce the
requested layers.

## Broker channel (protocol v8)

The host-service socket now multiplexes passengers beyond `db` and
`capability`:

| Passenger | What it mediates |
|---|---|
| `file` | read/write/delete/move/list within approved logical slots, saving downloads |
| `network` | HTTPS fetches with origin approval, redirect re-validation, SSRF blocking; downloads to the host temp area |
| `process` | child processes with timeouts, output caps, and an implicit-download ban (`npx -y`, `uvx`, `npm install`, …) |
| `credential` | host-owned credentials; only key names are ever audited |
| `artifact` | signed-catalog resolution, installation, and one-generation rollback |
| `platform` | time, locale, opening external URLs |
| `db` / `capability` | existing typed DB CRUD and plugin-to-plugin calls |

Identity is pinned to the authenticated token: a plugin can never open a
session as another plugin. Plugin binaries talk to brokers with the
`ene-plugin-broker` client; the host implements them in `ene-plugin-host`.

### Credential injection

API keys never travel to the plugin process. A plugin names a host-owned
credential on a network request (`credential: "api_key"` on `NetworkFetch`
/ `NetworkFetchStream`); the host resolves the value from its own state
(`plugins.list.<name>.credentials`, or the resolved
`ai.providers.<kind>.api_key` for built-in provider plugins), gates the
`CredentialUse` category, and injects `Authorization: Bearer <value>` into
the outgoing request. The plugin only ever sees the key name; the audit log
records the key name, never the value. A credential the host does not hold
fails the request before any network work, and authorization-like headers
sent by the plugin itself are stripped.

## Manifest layer

Every plugin has a signed manifest (`PluginManifest`) declaring:

- logical FS slots (`workspace`, `media`, …) — never real paths;
- fixed origins (exact `https://host[:port]`);
- `dynamic_web` (arbitrary public sites may be *requested*);
- required artifacts and sidecars (catalog ids + version constraints);
- host services it may open;
- declared side effects and resource ceilings;
- requestable permission categories and their maximum mode.

Capabilities that are not declared can never be approved, even by an
`Allow` policy. Built-in manifests ship inside the host binary and are
trusted by construction; third-party manifests must verify against a
trusted publisher key (`plugins.trusted_publishers`).

## Approval layer

Host-side policies decide what happens to a *declared* request:

1. **Mandatory constraints** — signature, hash, size, SSRF, containment.
   A violation is always denied.
2. **Per-plugin override** — wins over the global policy.
3. **Global policy** — per-category defaults (`Ask`).
4. **`Ask`** — interactive confirmation; headless consumers fail safe to
   deny.

Every category defaults to `Ask`. High-risk categories (file create/modify/
delete, HTTP, LAN, artifact installs/updates, process spawn, shell, browser,
credentials) show a persistent warning in the settings UI while set to
auto-allow and require a two-step confirmation the first time. The
**emergency stop** denies everything; **reset** restores `Ask` everywhere.

The settings page **Approvals** shows the global policy table, per-plugin
overrides, the high-risk warning, the emergency stop, the reset button, and
the audit-log path. Every decision — including automatic ones — is recorded
to the audit log (`app_data/audit/plugin-approval.jsonl`) with plugin,
category, target, applied rule, and outcome. Secrets, bodies, and file
contents are never logged.

## Downloads

Web browsing downloads and executable artifacts are completely separate:

- **Web files** are fetched by the host into its temp area, then the final
  URL, type, size, and SHA-256 are shown for confirmation before `FileBroker`
  saves them with a sanitized name. No automatic execution, extraction, or
  overwrite. An auto-save preset can skip the prompt only when a destination,
  size cap, and conflict rule are configured.
- **Executable artifacts** (plugins, sidecars, models) come only from the
  signed catalog: TUF-style metadata with Ed25519 signatures, expiry, and
  rollback protection; CAS (SHA-256) storage; resumable downloads with
  `Range`/`ETag`; atomic activation with one-generation rollback. Updates
  need approval per `artifact_id + version + digest + max_bytes`; same-version
  digest changes, expired catalogs, bad signatures, rollbacks, and size
  overruns are denied regardless of auto-allow.

## SSRF guard

The `Network` broker blocks loopback, private, link-local, cloud-metadata
(`169.254.169.254`), CGNAT, multicast, and reserved addresses, re-resolves
every host and pins the connection to the checked address (DNS-rebinding
protection), re-validates every redirect hop, and requires a fresh origin
approval for each hop. Plain HTTP gets a separate approval with a warning;
credentials are never injected over it. LAN access is denied in v1.
