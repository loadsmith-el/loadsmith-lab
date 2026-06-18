# Loadsmith Lab

> 📖 **Full documentation:** <https://loadsmith-el.github.io/loadsmith-lab/>

**The integration-test harness for [Loadsmith](https://github.com/loadsmith-el/loadsmith).**

Loadsmith Lab spins up real services in Docker, seeds them with canonical test
data, runs Loadsmith against them end-to-end, and validates the output. Each test
is a declarative *case*; the lab handles the Docker plumbing, readiness, and
verification so a pipeline can be proven against a real database — not a mock.

> If Loadsmith is the tool, the Lab is how you trust it. It's the recommended way
> to validate a pipeline or a new plugin against real infrastructure.

---

## Why a separate lab

- **Real services, not mocks.** A case runs against an actual `postgres:15`, seeded
  with 100k rows, on an isolated Docker network.
- **Canonical data.** Every service is seeded from the same deterministic dataset
  (`spacecraft_telemetry_events`, 34 columns, seed = 42), so results are
  comparable across services and reproducible across machines.
- **Declarative cases.** A case is two small YAML files — what to run and what to
  expect. No glue code.
- **Self-contained images.** Service images bake the seed data in at build time, so
  there's nothing to load at runtime beyond the database's own init.
- **Bring your own core/plugins.** Test a local Loadsmith and/or plugins with
  `--loadsmith <binary|project>` and `--plugin <binary|project>` (a project is
  built in a `rust:bookworm` container), or just use a packaged image — same
  cases either way. Plugins aren't bundled in the image; the lab installs the
  canonical set into a cache and mounts it, with `--plugin` overriding individual
  ones.

---

## Architecture

```text
loadsmith-lab-cli
  └─ loadsmith-lab-runner      orchestrates a case end-to-end
       ├─ loadsmith-lab-docker   thin async wrapper over bollard (Docker API)
       └─ loadsmith-lab-report   the elegant terminal output
```

| Crate | Responsibility |
|---|---|
| `loadsmith-lab-cli` | the `loadsmith-lab` binary: `run`, `build`, `list`, `origin`, `install` |
| `loadsmith-lab-runner` | origins/resolution, parse cases, start services, run Loadsmith, validate |
| `loadsmith-lab-docker` | image build/pull, container lifecycle, networks, TCP readiness |
| `loadsmith-lab-report` | banner, framed Loadsmith output, pass/fail summary |

### Three repos: engine, catalog, images

The lab binary ships **no content**. Cases, bundles, and service images live in
separate repos ("origins"), so you only pull in what you need:

```text
loadsmith-lab/                 ← this repo: the engine (4 crates), knows how, not what
loadsmith-lab-canonical-catalog/         ← cases/ + bundles/ + loadsmith-lab.toml (the "catalog" origin)
loadsmith-lab-canonical-images/          ← images/<name>/ (build contexts) + loadsmith-lab.toml
loadsmith-lab-canonical-data/  ← generate.py + schema (build-time data dependency, not an origin)
```

```text
loadsmith-lab-canonical-catalog/
  loadsmith-lab.toml          manifest: name → description, per category
  cases/<name>/               case.yaml (services + expectations) + pipeline.yaml
  bundles/<name>/             bundle.yaml + Dockerfile + scripts/
loadsmith-lab-canonical-images/
  loadsmith-lab.toml          manifest: name → description
  images/<name>/              Dockerfile + init — generates its seed CSV at build time
loadsmith-lab-canonical-data/
  generate.py                 the lone generator (stdlib-only, deterministic seed = 42)
  README.md                   the canonical 34-column schema contract
```

Everything is addressed as **`<origin>/<name>`** (e.g. `catalog/postgres-to-jsonl`,
`images/lab-postgres-15`) — the origin is part of the identity, so two origins can
ship the same name with no conflict.

**Origins** are either **remote** (a git repo, registered then `install`ed into a
local workdir) or **local** (a path on disk, read live — the dev workflow). A
case's service image is itself an `<origin>/<name>` image reference; the runner
resolves its build context from that origin, tries the Docker cache → a registry
pull → building the resolved `Dockerfile`. The image is responsible for its own
seed data: its Dockerfile clones `loadsmith-lab-canonical-data` (pinned) and runs
`generate.py` in a build stage, then bakes the CSV in — so **no large data file is
committed anywhere**. (Tradeoff: a cold image build needs network; runtime and
cached builds stay offline.)

See [docs/src/architecture/overview.md](docs/src/architecture/overview.md) for the
full origin/install model.

---

## How a case runs

```text
1. create an isolated Docker network
2. for each service: resolve image (cache → pull → build), start it
3. wait for readiness — TCP open AND, for SQL services, a query probe
     (so Loadsmith never connects before the 100k-row seed is loaded)
4. run Loadsmith (a local core via --loadsmith, or a published image) against
     the service
     · with a local core the service port is published and the pipeline's
       hostname is rewritten to 127.0.0.1
5. validate: exit status, output file exists, row counts match `expect`
6. tear everything down and report
```

The result is framed under each case — Loadsmith's own report (version, progress,
summary) shown inline, then the lab's verdict:

```text
loadsmith-lab 0.1.0  ·  local mode

▶ postgres-to-jsonl  Reads 100k rows from PostgreSQL into JSONL
  · pg ready · 127.0.0.1:15432 (2.0s)
  │ loadsmith 0.1.0
  │ postgres → jsonl
  │          2,000 rows · 1 batch
  │          ...
  │        100,000 rows · 50 batches
  │ Status:       success
  │ Throughput:   31,888 rows/s
  ✓ postgres-to-jsonl   100,000 read · 100,000 written   5.6s

──────────────────────────────────────────────────
1 passed, 0 failed
```

---

## Quickstart

The lab and its content repos are siblings of the `loadsmith` repo:

```text
projects/
  loadsmith/                ← the tool
  loadsmith-lab/            ← this repo (the engine)
  loadsmith-lab-canonical-catalog/    ← cases + bundles
  loadsmith-lab-canonical-images/     ← service images
```

```bash
# 1. (optional) sanity-check that Loadsmith compiles — the lab builds the core
#    image itself from `--loadsmith ../loadsmith` (a project dir → built in Docker)
cd ../loadsmith && cargo build

# 2. build the lab
cd ../loadsmith-lab && cargo build

# 3. register the catalog + images repos as LOCAL origins (read live, no install)
./target/debug/loadsmith-lab origin local add catalog ../loadsmith-lab-canonical-catalog
./target/debug/loadsmith-lab origin local add images  ../loadsmith-lab-canonical-images

# 4. run a case (auto-builds the service image on first run)
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
```

Requires Docker. The first run builds the `images/lab-postgres-15` image (seeded with
100k rows), which takes a moment; subsequent runs reuse it.

Local origins are read live from disk, so edits to a case/image take effect with
no re-install. To consume someone else's catalog instead, register it as a remote
origin and install what you need — see [Commands](#commands) and the
[docs](docs/src/architecture/overview.md).

---

## Commands

```bash
# Origins — where content comes from
loadsmith-lab origin list                              # all registered origins (remote + local)
loadsmith-lab origin local add catalog ../loadsmith-lab-canonical-catalog   # register a path, read live
loadsmith-lab origin remote add team https://github.com/acme/cases.git   # register + clone a git repo
loadsmith-lab origin show team                         # what an origin offers (its manifest)
loadsmith-lab origin remote update --all               # git pull every remote origin

# Install (remote origins only; local origins need no install)
loadsmith-lab install team/some-case                   # copy one item into the workdir
loadsmith-lab install team                             # install everything the origin offers
loadsmith-lab uninstall team/some-case
# installing a bundle also installs the cases it references (recursively, across origins)

# Run / list — names are always <origin>/<name>
loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl   # run one case (local binary)
loadsmith-lab run --all --loadsmith ../loadsmith                        # run every available case
loadsmith-lab run --select catalog/postgres-to-jsonl   # run against a Loadsmith container

loadsmith-lab list                                     # list available (installed + local) cases
loadsmith-lab list --available                         # also show not-yet-installed remote cases
loadsmith-lab build --select images/lab-postgres-15        # build a service image explicitly

loadsmith-lab bundle list                              # list available bundles
loadsmith-lab bundle run --loadsmith ../loadsmith --select catalog/parquet-destination   # run a bundle
loadsmith-lab bundle run --all --loadsmith ../loadsmith                 # run every bundle

# Global flags
--log-level debug    # forwarded to Loadsmith — shows the full protocol handshake inline
--no-color           # plain output for log files / CloudWatch
```

---

## Adding a service or case

New content goes in the **catalog** and **images** repos, not in this engine repo.
A case is two files plus a manifest entry:

1. `loadsmith-lab-canonical-catalog/cases/<name>/case.yaml` — services, readiness, `expect`
2. `loadsmith-lab-canonical-catalog/cases/<name>/pipeline.yaml` — the Loadsmith pipeline to run
3. add the case under `[cases]` in `loadsmith-lab-canonical-catalog/loadsmith-lab.toml`
4. if a new service image is needed, add `loadsmith-lab-canonical-images/images/<name>/`
   (a multi-stage Dockerfile + init that generates the seed CSV at build time —
   see the postgres reference) and an entry under `[images]` in
   `loadsmith-lab-canonical-images/loadsmith-lab.toml` — it's auto-built on first run

A case references its service image as an `<origin>/<name>` reference (e.g.
`image: images/lab-postgres-15`). With both repos registered as local origins, the new
content is picked up live — no install step.

### Convention: volume cases use the `null` destination

The 100k smoke case (`postgres-to-jsonl`) validates real content and the full
type round-trip. Any case that *inflates* the row count via a `CROSS JOIN` /
`generate_series` is a throughput/scale test, not a content test, and **must
use the `null` destination** (`loadsmith-destination-null`), which discards
batches and only counts rows. A multi-million-row JSONL is gigabytes
(5M ≈ 4.7 GB, 15M ≈ 15 GB), and the output directory defaults to the system
temp dir — often a tmpfs that can't hold it. `null` keeps volume tests off the
disk and isolates source+pump throughput.

Name volume cases `<service>-to-null-<N>` (e.g. `postgres-to-null-15M`), set
`destination.type: "null"` (quoted — `null` is a YAML keyword), and assert only
`rows_read`/`rows_written` in `expect` (no `output:` block).

> For validating output a case's `expect` block can't check (binary or
> multi-file output, e.g. Parquet), see **Bundles** below.

---

## Bundles

A **bundle** runs several cases as one hands-off sequence, each wrapped with
optional hook scripts: `setup` → run the case → `validate` → `cleanup`. It's how
you assert on output a case's built-in `expect` block can't — binary or
multi-file output like Parquet — by running a script (e.g. Python + pyarrow)
that opens the real produced files and checks them.

Cases are never modified by a bundle; they stay runnable standalone with
`run --select`. A bundle only chains and wraps them.

```
loadsmith-lab-canonical-catalog/bundles/<name>/
  bundle.yaml      ← the case sequence (each as <origin>/<name>) + hook script paths
  Dockerfile       ← builds the image the hooks run in
  scripts/         ← setup/validate/cleanup scripts
```

The hook scripts run **inside a container** built from the bundle's own
`Dockerfile`, so a bundle run needs **nothing installed on your host but
Docker** — no Python, no `pip install`. The shipped
`catalog/parquet-destination` bundle is a complete example (validates the
parquet destination in single-file and chunked modes).

```bash
loadsmith-lab bundle run --loadsmith ../loadsmith --select catalog/parquet-destination
```

See the [Writing Bundles guide](docs/src/writing-bundles/bundle-yaml.md) and the
[bundle.yaml schema](docs/src/reference/bundle-yaml-schema.md) for the full
contract.

---

## Canonical data

The canonical dataset is **100,000 rows of spacecraft telemetry, 34 columns,
deterministic (`seed = 42`)**, produced by `generate.py` in the
[`loadsmith-lab-canonical-data`](https://github.com/loadsmith-el/loadsmith-lab-canonical-data) repo. Because
it's a pure function of that stdlib-only generator, **no CSV is committed
anywhere** — each image's Dockerfile clones the data repo (pinned tag) and runs
`generate.py` in a build stage, baking the result in.

To change the dataset, edit `generate.py` in `loadsmith-lab-canonical-data`, bump
its tag, and point images at the new `DATA_REF`. To preview it locally:

```bash
cd ../loadsmith-lab-canonical-data && python generate.py   # → spacecraft_telemetry_events.csv (gitignored)
```

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
