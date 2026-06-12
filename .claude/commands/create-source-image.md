---
description: Create a new lab source-service image (Dockerfile + init + smoke case)
argument-hint: <service> <version> [data-ref] [notes]
allowed-tools: [Read, Write, Bash]
---

Scaffold everything needed to add a new **source service** to loadsmith-lab: a
Docker image that boots the service pre-seeded with the canonical test data (the
service a Loadsmith pipeline reads *from*), plus a smoke-test case that reads 100k
rows out of it into JSONL.

The user invoked this command with: $ARGUMENTS

## Read these first (ground truth — do not skip)

Content lives in two sibling repos (the engine ships none): images go in
`loadsmith-lab-canonical-images`, cases in `loadsmith-lab-canonical-catalog`. Read the existing
postgres image and case as the working reference. The new scaffold must mirror
their structure exactly:

1. `../loadsmith-lab-canonical-images/images/lab-postgres-15/Dockerfile` — how an image is built (note: **no DuckDB, no Parquet**)
2. `../loadsmith-lab-canonical-images/images/lab-postgres-15/init.sql` — the canonical 34-column schema + bulk load + indexes
3. `../loadsmith-lab-canonical-catalog/cases/postgres-to-jsonl/case.yaml` — service definition + **readiness probe** + expectations
4. `../loadsmith-lab-canonical-catalog/cases/postgres-to-jsonl/pipeline.yaml` — the loadsmith source→jsonl pipeline
5. `crates/loadsmith-lab-runner/src/case.rs` — the case schema you must conform to
   (especially `ReadinessDef` / `PostgresReadiness`)

Also peek at the data so types match reality (generate it once locally; it's
gitignored):

```bash
(cd ../loadsmith-lab-canonical-data && python generate.py) \
  && head -2 ../loadsmith-lab-canonical-data/spacecraft_telemetry_events.csv
```

## Architecture you MUST respect

- **The seed CSV is generated at build time — never committed.** The canonical
  100k-row, 34-column dataset (deterministic, stdlib-only) is produced by
  `generate.py` in the `loadsmith-lab-canonical-data` repo. There is no Parquet
  and no DuckDB anywhere. Do not add either, and do **not** commit a CSV into the
  image directory.
- **The Dockerfile is multi-stage.** A `data` stage clones
  `loadsmith-lab-canonical-data` (pinned `DATA_REF`) and runs `generate.py`; the
  service stage does `COPY --from=data /gen/spacecraft_telemetry_events.csv
  /…/events.csv`. The lab tars only the image dir's files (`Dockerfile`,
  `init.sql`) — it injects nothing. Mirror `lab-postgres-15/Dockerfile` exactly.
- **Naming.** The image item is `images/<name>` (origin/name), built under the
  local tag `loadsmith-lab/images/<name>:local`. The item name *is* the directory
  name — no prefix stripping. Add an entry under `[images]` in
  `../loadsmith-lab-canonical-images/loadsmith-lab.toml`, and under `[cases]` in
  `../loadsmith-lab-canonical-catalog/loadsmith-lab.toml`.
- **Credentials are always `lab` / `lab` / `lab`** (user / password / database or
  index), set via the image's native env vars, so every case is uniform.
- **Empty string means NULL.** The CSV encodes nulls as empty fields; the loader
  must treat empty as NULL (postgres uses `NULL ''` in its `COPY`).

## Steps

### 1. Parse arguments
- **service** — e.g. `mysql`, `mariadb`, `clickhouse`, `cockroachdb`, `mongodb`
- **version** — the *service* version, e.g. `8`, `15`, `latest`
- **data-ref** — optional canonical-data revision to bake in (the image's
  `ARG DATA_REF`). This is an **independent, per-image choice**, decoupled from
  the service version — e.g. you can bump the service base for a security patch
  while keeping the data on an older revision. **Default**: the latest
  canonical-data tag, found with
  `git -C ../loadsmith-lab-canonical-data tag --sort=-v:refname | head -1`
  (fall back to `git ls-remote --tags https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git`
  if the sibling repo isn't present). Use whatever the user passed if given.
- **notes** — optional extra context

### 2. `../loadsmith-lab-canonical-images/images/<service>-<version>/Dockerfile`
- **Global ARGs** (before the first `FROM`):
  `ARG DATA_REPO=https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git`
  and `ARG DATA_REF=<data-ref>` (the revision resolved in step 1). Declared
  globally so both stages see them.
- **Stage 1 (`AS data`)**: `FROM python:3-slim`, install git, re-declare
  `ARG DATA_REPO` / `ARG DATA_REF` (no value — inherits the global default),
  then
  `RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen && python /gen/generate.py`.
  Mirror `lab-postgres-15/Dockerfile` exactly.
- **Stage 2**: `FROM <official-image>:<version>`, then re-declare `ARG DATA_REF`
  and add `LABEL org.opencontainers.image.version="${DATA_REF}"` so the image
  self-reports its baked-in data revision (the CI derives the `:data-<ref>` tag
  from the same `DATA_REF`).
- `COPY --from=data /gen/spacecraft_telemetry_events.csv <path>/events.csv` —
  wherever the init mechanism can reach it (never commit a CSV)
- `COPY` your init file into the image's native init hook (e.g. an entrypoint
  init dir, or a script run on first boot)
- Set the `lab`/`lab`/`lab` credentials + db/index name via the image's env vars
- **No package installs for data conversion.** The CSV is the input format. If the
  target genuinely cannot ingest CSV (e.g. a document store), convert CSV→target
  format with a tiny inline script in the init step — but prefer the service's
  native CSV bulk loader (`LOAD DATA INFILE`, `COPY`, `clickhouse-client
  --query`, etc.).

### 3. `../loadsmith-lab-canonical-images/images/<service>-<version>/init.<ext>`
- Recreate the **exact** `spacecraft_telemetry_events` schema from
  `lab-postgres-15/init.sql`, mapping each of the 34 columns to the target's
  closest native type (preserve NOT NULL / NULL and decimal precision/scale where
  the engine supports it).
- Bulk-load `events.csv` (header row present, empty = NULL).
- Add the same secondary indexes where the engine supports them.

### 4. `../loadsmith-lab-canonical-catalog/cases/<service>-<version>-to-jsonl/case.yaml`
Mirror `cases/postgres-to-jsonl/case.yaml`. Critical details:

```yaml
case:
  name: <service>-<version>-to-jsonl
  description: <one line>
  tags: [<service>, jsonl, smoke]

services:
  - image: images/<service>-<version>   # <origin>/<name> image reference
    alias: <short-alias>          # e.g. mysql, ch, mongo
    readiness:
      tcp: <default-port>
      timeout_seconds: 300        # init loads 100k rows — give it room
      # See "Readiness gating" below — add a query-level probe if possible.
    env:
      - <ANY_REQUIRED_ENV=value>  # only if the image needs it at runtime

# `image` is the published image for the default (remote) path; --loadsmith <path>
# builds/wraps a local core and ignores it. loadsmith ALWAYS runs in a
# container — never a host binary. The output dir is mounted at /output
# automatically, so don't declare a /output volume; `volumes` is only for extras.
loadsmith:
  image: loadsmith:latest

pipeline:
  file: pipeline.yaml

expect:
  status: success
  rows_read: 100000
  rows_written: 100000
  output:
    file: events.jsonl     # resolved under the host side of the /output mount
    row_count: 100000
```

### 5. `../loadsmith-lab-canonical-catalog/cases/<service>-<version>-to-jsonl/pipeline.yaml`
Mirror `cases/postgres-to-jsonl/pipeline.yaml`, using the correct `source.type`
and connection config for the service. Connect as `host: <alias>` (the Docker
alias — loadsmith shares the network and reaches it directly; no rewriting), port
the service's default, db `lab`, creds `lab`/`lab`. Destination is `jsonl` writing
to `/output/events.jsonl`. Use a stable `ORDER BY` (e.g. `event_sequence`) so row
order is deterministic.

## Volume / scale cases use the `null` destination

The smoke case above reads the canonical 100k rows into JSONL — that one case
validates content and the full type round-trip. **Any case that inflates the row
count beyond 100k via a `CROSS JOIN` (or `generate_series`) is a throughput/scale
test, not a content test, and its destination MUST be `null`**
(`loadsmith-destination-null`) — it discards every batch and only counts rows.

Why: a multi-million-row JSONL is gigabytes (5M ≈ 4.7 GB, 15M ≈ 15 GB) and the
lab's output dir defaults to the system temp dir, which is often a tmpfs that
can't hold it. The `null` sink keeps volume tests off the disk entirely and
isolates source + pump throughput. Convention for these:

- Name: `<service>-to-null-<N>` (e.g. `postgres-to-null-15M`).
- Pipeline destination: `type: "null"` (quote it — bare `null` is YAML null), no
  `config`.
- `expect`: assert only `rows_read` / `rows_written` — there is no output file, so
  no `output:` block.

```yaml
# pipeline.yaml — volume variant
source:
  type: postgres
  config:
    # …
    query: >
      SELECT e.*, (e.event_sequence + (g.n * 100000)) AS synthetic_sequence
      FROM spacecraft_telemetry_events e
      CROSS JOIN generate_series(0, 149) AS g(n)   # 150 × 100k = 15M
    batch_size: 5000
destination:
  type: "null"
```

## Readiness gating (the lesson that bites)

TCP-open is **not** "ready". The service accepts connections while it is still
loading 100k rows, so loadsmith can connect and read an empty table. Guard against
it:

- **Postgres-protocol services** (postgres, cockroachdb, yugabyte): add a
  `postgres:` probe block under `readiness` that waits for data, exactly like the
  postgres case:
  ```yaml
      postgres:
        dbname: lab
        user: lab
        password: lab
        probe_query: "SELECT 1 FROM spacecraft_telemetry_events LIMIT 1"
  ```
- **Other services**: `ReadinessDef` currently only implements a postgres probe
  (see `case.rs`). For now rely on `tcp` + a generous `timeout_seconds`. If the
  service routinely races, note in your summary that
  `crates/loadsmith-lab-runner/src/runner.rs` needs an analogous query probe for
  this engine — don't silently ship a flaky case.

## Verify before finishing

- The case's `image:` reference matches the image directory: `images/<service>-<version>`
  ↔ `../loadsmith-lab-canonical-images/images/<service>-<version>/`.
- The Dockerfile is multi-stage: a `data` stage clones the canonical-data repo +
  runs `generate.py`; the service stage does `COPY --from=data … events.csv`.
- **No CSV is committed** in the image directory (it's generated at build time).
- `DATA_REF` is a global `ARG` and the final stage carries
  `LABEL org.opencontainers.image.version="${DATA_REF}"` (no hand-written
  VERSION file — the CI derives the `:data-<ref>` tag from `DATA_REF`).
- Both manifests updated: `[images]` in `loadsmith-lab-canonical-images/loadsmith-lab.toml`,
  `[cases]` in `loadsmith-lab-canonical-catalog/loadsmith-lab.toml`.
- The init schema has all 34 columns in the same order as the CSV header.
- No DuckDB, no Parquet, no apt-get for data tooling.

## After creating the files

Print a short summary of what was created, then tell the user the next step is to
run the case — it auto-builds the image on first run (assumes the catalog/images
repos are registered as local origins):

```bash
loadsmith-lab run --loadsmith ../loadsmith --select catalog/<service>-<version>-to-jsonl
```

Do NOT run any build or run commands yourself — only scaffold the files.
