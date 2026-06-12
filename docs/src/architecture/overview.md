# Architecture Overview

loadsmith-lab is a Rust workspace with four crates in a strict dependency hierarchy.

## Crate map

```
loadsmith-lab-cli                   ← the loadsmith-lab binary (clap)
  └─ loadsmith-lab-runner           ← orchestrates cases: network, containers, loadsmith, validation
       └─ loadsmith-lab-docker      ← async Docker API wrapper (bollard)
       └─ loadsmith-lab-report      ← terminal output: banner, case headers, framed results
```

The CLI knows about commands and flags. The runner knows about case lifecycle. The Docker crate knows about containers. The report crate knows about terminal formatting. None of these layers bleeds into the other.

## Three repos: engine, catalog, images

The engine ships **no content**. Cases, bundles, and service images live in
separate repos ("origins"); the engine resolves them on demand.

```
loadsmith-lab/                            ← THIS repo: the engine (4 crates)
  Cargo.toml                              ← workspace root
  crates/
    loadsmith-lab-cli/
    loadsmith-lab-runner/                 ← incl. origin.rs (origin/install/resolution)
    loadsmith-lab-docker/
    loadsmith-lab-report/

loadsmith-lab-canonical-catalog/                    ← the "catalog" origin
  loadsmith-lab.toml                      ← manifest: name → description per category
  cases/postgres-to-jsonl/
    case.yaml                             ← what to run and what to assert
    pipeline.yaml                         ← the loadsmith pipeline
  bundles/<name>/                         ← bundle.yaml + Dockerfile + scripts/

loadsmith-lab-canonical-images/                     ← the "images" origin
  loadsmith-lab.toml                      ← manifest: name → description
  images/lab-postgres-15/                     ← build context (no committed data)
    Dockerfile                            ← multi-stage: clones the data repo + generates the CSV
    init.sql

loadsmith-lab-canonical-data/             ← build-time data dependency (NOT an origin)
  generate.py                             ← the lone generator (stdlib-only, seed=42)
  README.md                               ← the canonical 34-column schema contract
```

The CSV is **not committed** anywhere: it's a pure function of the deterministic
`generate.py`, so each image regenerates it at build time (see
[The Docker Model](./docker-model.md)).

## Origins, manifests & install

Everything is addressed as **`<origin>/<name>`** — no bare names, no collision
rules. Two origins can ship the same name with no conflict.

- **Manifest** — each origin has a `loadsmith-lab.toml` at its root: a minimal
  index (name → short description) under `[cases]` / `[bundles]` / `[images]`.
  It doesn't redefine paths — `cases.foo` always lives at `cases/foo/`. It's
  what `origin show` and `list --available` read, without scanning the repo.

- **Remote origin** (git) — registered with `origin remote add <name> <url>`,
  shallow-cloned into the cache dir (`~/.cache/loadsmith-lab/origins/<name>/`).
  Its content is **not** usable until `install`ed: `install <origin>/<name>`
  copies one item (or `install <origin>` copies all) from the cache clone into
  the workdir at `~/.local/share/loadsmith-lab/installed/<kind>/<origin>/<name>/`.
  `origin remote update` refreshes the clone; `origin list` does a lightweight
  `git ls-remote` check and flags origins with a new version available.

- **Local origin** (path) — registered with `origin local add <name> <path>`,
  read **live** in place. No clone, no install, no copy — edits take effect
  immediately. This is the dev workflow (register the sibling catalog/images
  checkouts). `generate` only works against a local origin (it rewrites the
  CSV in place).

Config lives in `~/.config/loadsmith-lab/origins.toml`. The `catalog`/`images`
defaults are seeded there on first run (as remote git origins) but not cloned
until first use.

`origin.rs` (in `loadsmith-lab-runner`) owns all of this: `Config`/`Manifest`,
`resolve_item`, `discover`, `install`/`uninstall`, the git shell-outs, and the
`<origin>/<name>` → `loadsmith-lab/<origin>/<name>:local` image-tag derivation.

## How a case runs

`run_case()` in `loadsmith-lab-runner` executes these steps in order:

```
1. parse case.yaml
2. create isolated Docker bridge network (UUID name: "ls-lab-<uuid>")
3. for each service:
     a. resolve the service's <origin>/<name> image to a build context (origin.rs),
        then resolve the image (cache → pull → build that context)
     b. start container
     c. wait for TCP port to be open
     d. if postgres readiness configured: wait for probe_query to return rows
4. resolve the loadsmith image:
     --loadsmith <dir>:    build loadsmith:local from that project's Dockerfile
     --loadsmith <binary>: wrap the binary in a minimal image
     default:              pull the canonical image ghcr.io/loadsmith-el/loadsmith
                           (--tag <t> picks the version, else :slim)
   then prepare the plugin dir: the cached canonical set (loadsmith plugin
   install --all) plus any --plugin overlays (binary, or a crate/workspace built
   in a rust:bookworm container)
5. run loadsmith in a container on the network, mounting the pipeline + /output +
   the plugin dir at /plugins; stream its output through the gutter live
6. validate expectations (exit code, row counts, output file)
7. tear down containers
8. tear down network
9. return CaseResult
```

Teardown happens via `scopeguard::defer` — even if any step panics or returns
an error, the network and containers are cleaned up.

## Key data structures

**`Case`** — the deserialized `case.yaml`:
```rust
pub struct Case {
    pub case: CaseMeta,
    pub services: Vec<ServiceDef>,
    pub loadsmith: LoadsmithDef,
    pub pipeline: PipelineDef,
    pub expect: Expect,
}
```

**`RunOpts`** — options passed to `run_case`:
```rust
pub struct RunOpts {
    pub loadsmith_source: Option<PathBuf>, // --loadsmith: a binary or a project dir
    pub loadsmith_tag: Option<String>,     // published image when loadsmith_source is None
    pub origins: Config,                    // resolves a service's <origin>/<name> image
    pub color: bool,
    pub log_level: String,
    pub plugin_overrides: Vec<PathBuf>,     // --plugin: local binaries/projects
}
```

**`CaseResult`** — what comes back:
```rust
pub struct CaseResult {
    pub name: String,
    pub description: Option<String>,
    pub passed: bool,
    pub duration: Duration,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    pub prep_lines: Vec<String>,      // image build/pull messages, readiness status
    pub loadsmith_output: String,     // loadsmith's stdout, shown framed
    pub failures: Vec<String>,        // what assertions failed
}
```
