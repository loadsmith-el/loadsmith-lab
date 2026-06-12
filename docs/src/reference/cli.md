# CLI Reference

The `loadsmith-lab` binary is the entry point for all lab operations.

Cases, bundles, and images are addressed as **`<origin>/<name>`** (e.g.
`catalog/postgres-to-jsonl`, `images/lab-postgres-15`). See
[Origins, manifests & install](../architecture/overview.md) for the model and
the `origin`/`install` commands below.

## Global flags

| Flag | Default | Description |
|---|---|---|
| `--log-level <level>` | `info` | Lab's own log verbosity (stderr). Also forwarded to loadsmith |
| `--no-color` | off | Disable ANSI color in all output. Equivalent to `NO_COLOR=1` |

## `loadsmith-lab run`

Run one or more test cases.

```bash
loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
loadsmith-lab run --loadsmith ../loadsmith --all
loadsmith-lab run --loadsmith ../loadsmith --all --log-level debug
loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl --no-color
loadsmith-lab run --loadsmith ../loadsmith --all --cases-dir /path/to/cases
```

**Flags:**

| Flag | Default | Description |
|---|---|---|
| `--select <origin>/<name>` | — | Run only this case. Mutually exclusive with `--all` |
| `--all` | — | Run all available cases (installed + local origins). Mutually exclusive with `--select` |
| `--loadsmith <path>` | — | Run a local loadsmith core: a binary (wrapped in a minimal image) or a project dir (built from its Dockerfile). Without it, the canonical published image `ghcr.io/loadsmith-el/loadsmith` is pulled |
| `--plugin <path>` | — | Override a cached canonical plugin with a local one: a binary, a plugin crate, or a workspace root (built in a `rust:bookworm` container). Repeatable |
| `--tag <tag>` | — | Version tag of the canonical image `ghcr.io/loadsmith-el/loadsmith:<tag>` (e.g. `v0.1.0-slim`; default `slim`). Ignored when `--loadsmith` is given |
| `--cases-dir <path>` | — | Ad-hoc: resolve cases directly from this dir (bare `<name>` subdirs), bypassing origins |

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | All selected cases passed |
| `1` | One or more cases failed or errored |

**What `--log-level` does:**

The `--log-level` flag controls two things:
1. The lab's own tracing verbosity (goes to stderr).
2. The `--log-level` argument passed to loadsmith inside the container.

At `debug` or `trace`, the full protocol handshake appears inside the framed
loadsmith output in the report.

---

## `loadsmith-lab bundle`

Run or list test bundles — sequenced cases wrapped with setup/validate/cleanup
hook scripts. See [Writing Bundles](../writing-bundles/bundle-yaml.md).

### `loadsmith-lab bundle run`

```bash
loadsmith-lab bundle run --loadsmith ../loadsmith --select catalog/parquet-destination
loadsmith-lab bundle run --loadsmith ../loadsmith --all
```

**Flags:**

| Flag | Default | Description |
|---|---|---|
| `--select <origin>/<name>` | — | Run only this bundle. Mutually exclusive with `--all` |
| `--all` | — | Run all available bundles. Mutually exclusive with `--select` |
| `--loadsmith <path>` | — | Run a local loadsmith core: a binary (wrapped in a minimal image) or a project dir (built from its Dockerfile). Without it, the canonical published image `ghcr.io/loadsmith-el/loadsmith` is pulled |
| `--plugin <path>` | — | Override a cached canonical plugin with a local one: a binary, a plugin crate, or a workspace root (built in a `rust:bookworm` container). Repeatable |
| `--tag <tag>` | — | Version tag of the canonical image `ghcr.io/loadsmith-el/loadsmith:<tag>` (e.g. `v0.1.0-slim`; default `slim`). Ignored when `--loadsmith` is given |
| `--bundles-dir <path>` | — | Ad-hoc: resolve bundles directly from this dir, bypassing origins |

A bundle's entries reference their cases as `<origin>/<name>` too, resolved the
same way (installed copy or live local origin).

The hook scripts run inside an image built from each bundle's `Dockerfile`
(tagged `loadsmith-lab-bundle-<name>:local`), so no host interpreter or
dependencies are required — only Docker.

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | Every entry in every selected bundle passed |
| `1` | One or more entries failed or a bundle errored |

### `loadsmith-lab bundle list`

```bash
loadsmith-lab bundle list
```

**Flags:**

| Flag | Default | Description |
|---|---|---|
| `--bundles-dir <path>` | — | Ad-hoc: list bundles directly from this dir, bypassing origins |

Bundles are discovered as `bundle.yaml`-bearing subdirectories across the
workdir (installed) and registered local origins, listed as `<origin>/<name>`.

---

## `loadsmith-lab list`

List all available cases (installed copies + live local origins), as
`<origin>/<name>`.

```bash
loadsmith-lab list
loadsmith-lab list --available
loadsmith-lab list --cases-dir /path/to/cases
```

**Flags:**

| Flag | Default | Description |
|---|---|---|
| `--available` | off | Also show not-yet-installed cases offered by remote origins' manifests |
| `--cases-dir <path>` | — | Ad-hoc: list cases directly from this dir, bypassing origins |

**Output:**
```
catalog/postgres-to-jsonl
team/mysql-to-parquet
```

---

## `loadsmith-lab build`

Pre-build lab service images without running any case.

```bash
loadsmith-lab build --select images/lab-postgres-15
loadsmith-lab build --all
```

**Flags:**

| Flag | Default | Description |
|---|---|---|
| `--select <origin>/<name>` | — | Build this image (e.g. `images/lab-postgres-15`) |
| `--all` | — | Build all available images (installed + local origins) |

Each image builds under the local tag `loadsmith-lab/<origin>/<name>:local`
(e.g. `loadsmith-lab/images/lab-postgres-15:local`). Build follows the resolution
order: an image already in the local Docker cache is not rebuilt. Remove it
first to force a rebuild:

```bash
docker rmi loadsmith-lab/images/lab-postgres-15:local
loadsmith-lab build --select images/lab-postgres-15
```

Service images generate their canonical seed CSV at build time (a Dockerfile
build stage clones `loadsmith-lab-canonical-data` and runs `generate.py`) — there
is no `loadsmith-lab generate` command. See
[Canonical Test Data](../canonical-data/overview.md).

---

## `loadsmith-lab origin`

Manage origins — where cases/bundles/images come from.

```bash
loadsmith-lab origin list                       # all origins (remote + local), with update hints
loadsmith-lab origin show <name>                # print an origin's manifest

loadsmith-lab origin remote add <name> <url>    # register a git origin + clone it
loadsmith-lab origin remote update [<name>|--all]   # git pull (refresh the cache clone)
loadsmith-lab origin remote rm <name> [--purge]     # deregister (--purge also deletes the clone)
loadsmith-lab origin remote list

loadsmith-lab origin local add <name> <path>    # register a path, read live (no install)
loadsmith-lab origin local rm <name>
loadsmith-lab origin local list
```

`origin list` runs a lightweight `git ls-remote` check per remote origin and
prints `new version available` for any whose remote has moved ahead of the local
clone. Connectivity failures are silently skipped (never an error). The default
`catalog`/`images` origins are seeded into `origins.toml` on first run but only
cloned on first use.

## `loadsmith-lab install` / `uninstall`

Copy a remote origin's content into the local workdir (local origins need no
install — they're read live).

```bash
loadsmith-lab install <origin>/<name>     # install one item
loadsmith-lab install <origin>            # install everything the origin offers
loadsmith-lab uninstall <origin>/<name>   # remove an installed item
```

`install` is overwrite (idempotent), not additive — re-running it after an
`origin remote update` refreshes the installed copy.

Installing a bundle also installs every case it references in `bundle.yaml`,
recursively and across origins (cases from local/path origins are skipped —
they're read live). This means a freshly registered remote origin only needs
`install <origin>/<bundle-name>` before `bundle run` works.

---

## Environment variables

| Variable | Equivalent | Description |
|---|---|---|
| `NO_COLOR` | `--no-color` | Disable ANSI color (any non-empty value) |

Origin/config/cache/workdir locations honour the XDG base-directory variables
(`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`).
