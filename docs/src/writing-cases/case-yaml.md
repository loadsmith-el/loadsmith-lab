# case.yaml

Each case lives in its own directory under `cases/` in the
`loadsmith-lab-canonical-catalog` repo (the "catalog" origin), addressed as
`catalog/<name>`. A case directory contains at least `case.yaml` and its
pipeline file, plus any fixtures it mounts (through a relative
`loadsmith.volumes[].host`):

```
loadsmith-lab-canonical-catalog/cases/my-case/
  case.yaml      ← what services to start and what to assert
  pipeline.yaml  ← the loadsmith pipeline to run (name set by pipeline.file)
  tls/           ← optional fixtures, e.g. a TLS wallet (oracle-to-jsonl-tls)
```

Add a matching entry under `[cases]` in `loadsmith-lab-canonical-catalog/loadsmith-lab.toml`
so the case appears in the origin's manifest.

## Full structure

```yaml
case:
  name: string           # required
  description: string    # optional
  tags: [string]         # optional

services:                # required — may be []
  - image: string        # required — <origin>/<name>, e.g. images/lab-postgres-15
    alias: string        # required — hostname in the Docker network
    readiness:           # optional
      tcp: integer       # port to wait for
      timeout_seconds: integer  # default: 60 (per stage)
      postgres:          # optional — additional Postgres-level probe
        dbname: string
        user: string
        password: string
        probe_query: string  # optional — default: "SELECT 1"
      mysql:             # optional — additional MySQL-level probe
        dbname: string
        user: string
        password: string
        probe_query: string  # optional — default: "SELECT 1"
      oracle:            # optional — additional Oracle-level probe
        service_name: string
        user: string
        password: string
        probe_query: string  # optional — default: "SELECT 1 FROM dual"
    env: [string]        # optional — "KEY=VALUE" strings
    docker_args: [string] # optional — used as bind-mount specs (host:container)

loadsmith:               # optional block
  image: string          # optional — full image-ref override (default: ghcr.io/loadsmith-el/loadsmith:slim)
  volumes:               # optional
    - host: string
      container: string
  env: [string]          # optional
  docker_args: [string]  # optional

pipeline:
  file: string           # path to pipeline.yaml, relative to the case directory

expect:
  status: string         # "success" or "error"
  exit_code: integer     # optional — exact loadsmith exit code (e.g. 3)
  rows_read: integer     # optional — exact rows_read asserted
  rows_written: integer  # optional — exact rows_written asserted
  output:                # optional
    file: string         # path to output file
    row_count: integer   # optional — number of lines expected in the file
  logs:                  # optional
    not_contains: [string]  # optional — must not appear in loadsmith's output
    contains: [string]      # optional — must appear in loadsmith's output
```

## Section reference

### `case`

Metadata about the case.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Display name in the result line. `--select` and `loadsmith-lab list` use `<origin>/<directory name>` instead, so keep it equal to the directory name by convention |
| `description` | string | no | Shown in the case header when running |
| `tags` | list of strings | no | Arbitrary tags. Unused — reserved for future filtering |

### `services`

A list of Docker services to start before running loadsmith. Each service is
started in order and must pass its readiness check before the next one starts.
The `services:` key is required; use `services: []` for a case with none.

| Field | Type | Required | Description |
|---|---|---|---|
| `image` | string | yes | Service image as an `<origin>/<name>` reference (e.g. `images/lab-postgres-15`), built and tagged locally as `loadsmith-lab/<origin>/<name>:local`. Public images can't be referenced directly — see [Adding service images](adding-service-images.md) |
| `alias` | string | yes | Hostname assigned to the container in the Docker network. Use this in `pipeline.yaml` |
| `readiness.tcp` | integer | no | Port number to wait for. Polled every 500 ms |
| `readiness.timeout_seconds` | integer | no | Timeout per readiness stage: the TCP wait and each query probe each get the full value. Default: 60 |
| `readiness.postgres` | object | no | Additional Postgres-level readiness probe |
| `readiness.mysql` | object | no | Additional MySQL-level readiness probe |
| `readiness.oracle` | object | no | Additional Oracle-level readiness probe |
| `env` | list of strings | no | Environment variables passed to the container (`"KEY=VALUE"`) |
| `docker_args` | list of strings | no | Despite the name, each entry is used as a bind-mount spec (`host:container[:opts]`), not a docker run flag |

### `readiness.postgres`

After the TCP port is open, the Postgres readiness probe connects to the database
and runs `probe_query` until it returns at least one row. This handles a critical
timing issue: PostgreSQL's `COPY` is transactional, meaning the TCP port is
available long before all rows are committed and visible to queries.

Without the Postgres probe, loadsmith could start before the table has any data.

| Field | Type | Required | Description |
|---|---|---|---|
| `dbname` | string | yes | Database name |
| `user` | string | yes | Login user |
| `password` | string | yes | Login password |
| `probe_query` | string | no | SQL query that must return ≥1 row. Default: `"SELECT 1"` |

**Recommended probe query for seeded tables:**

```yaml
probe_query: "SELECT 1 FROM spacecraft_telemetry_events LIMIT 1"
```

This confirms that at least one row has been committed, which means the `COPY`
statement in `init.sql` has completed.

### `readiness.mysql`

The same idea for MySQL: after the TCP port is open, connect and run
`probe_query` until it returns at least one row, so loadsmith doesn't start
before the init script's `LOAD DATA` has finished.

| Field | Type | Required | Description |
|---|---|---|---|
| `dbname` | string | yes | Database name |
| `user` | string | yes | Login user |
| `password` | string | yes | Login password |
| `probe_query` | string | no | SQL query that must return ≥1 row. Default: `"SELECT 1"` |

### `readiness.oracle`

Oracle takes minutes to open the PDB and load the seed, so the Oracle cases rely
on this probe (with a large `timeout_seconds`). It connects by service name
instead of a database name.

| Field | Type | Required | Description |
|---|---|---|---|
| `service_name` | string | yes | Oracle service name (e.g. `FREEPDB1`) |
| `user` | string | yes | Login user |
| `password` | string | yes | Login password |
| `probe_query` | string | no | SQL query that must return ≥1 row. Default: `"SELECT 1 FROM dual"` |

```yaml
oracle:
  service_name: FREEPDB1
  user: lab
  password: lab
  probe_query: "SELECT 1 FROM spacecraft_telemetry_events WHERE ROWNUM = 1"
```

### `loadsmith`

How to run loadsmith for this case.

The whole `loadsmith:` block is **optional** — omit it unless the case needs
extra mounts/env/args or a non-default image.

| Field | Type | Required | Description |
|---|---|---|---|
| `image` | string | no | Override the loadsmith image with a full ref. Normally omitted — the lab pulls the canonical `ghcr.io/loadsmith-el/loadsmith` (`--tag` picks the version, else `:slim`) |
| `volumes` | list | no | Extra bind mounts. Each entry has `host` and `container` paths. A relative `host` is resolved against the case directory (for committed fixtures); an absolute path is used as-is |
| `env` | list of strings | no | Environment variables for the loadsmith process |
| `docker_args` | list of strings | no | Currently ignored — accepted but never applied to the loadsmith container |

Loadsmith always runs in a container. By default the lab pulls the canonical
published image; `--tag <tag>` picks the version and `--loadsmith <path>`
builds/wraps a local core instead (both ignore a case's `image`). The output
directory is always mounted at `/output` automatically, so `volumes` is only for
*additional* mounts (don't remap `/output`).

### `pipeline`

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | Path to the pipeline YAML, relative to the case directory |

### `expect`

Assertions validated after the run. A case fails if any assertion does not hold.

| Field | Type | Required | Description |
|---|---|---|---|
| `status` | string | yes | Expected outcome. `"success"` or `"error"` |
| `exit_code` | integer | no | Expected loadsmith exit code (`0`, `1`, `2`, `3`, `4`, `5`, `130`, `143`), e.g. `3` for a configuration error |
| `rows_read` | integer | no | Expected `rows_read` from the loadsmith run summary |
| `rows_written` | integer | no | Expected `rows_written` from the loadsmith run summary |
| `output.file` | string | no | Output file to check. Relative paths resolve under the host side of the `/output` mount |
| `output.row_count` | integer | no | Expected number of lines in the output file |
| `logs.not_contains` | list of strings | no | Substrings that must appear in **none** of the loadsmith container's stdout, stderr or JSON run summary — e.g. a password, to prove it never leaks |
| `logs.contains` | list of strings | no | Substrings that must appear in at least one of them |

Status, exit code and row counts come from loadsmith's JSON run summary
(`--summary-file`). With an engine that predates it (the published `v0.2.0`
images), row counts are parsed from the summary box printed to stdout instead
(case-insensitive, comma-formatted numbers such as `100,000`). See
[How the lab reads the run](../getting-started/understanding-output.md#how-the-lab-reads-the-run).

### Proving a secret never leaks

`expect.logs` asserts on what the loadsmith container printed. ANSI colour codes
are stripped before matching, and stdout, stderr and the run summary file are
each searched:

```yaml
expect:
  status: success
  logs:
    not_contains: ["s3cr3t-value", "BEGIN PRIVATE KEY"]   # neither stdout, stderr nor the summary
    contains: ["schema negotiated"]                        # optional positive check
```

A `not_contains` needle is a secret, so the lab never prints it: a failure reads
`logs.not_contains: needle "s3cr…" (12 chars) found in stderr`, and the needle is
masked (`[redacted "s3cr…" (12 chars)]`) in the live loadsmith output and in the
other failure lines. A misspelt key under `logs:` is an error, not a silent pass.
See [Validation behavior](../reference/case-yaml-schema.md#validation-behavior).

## Complete example

```yaml
case:
  name: postgres-to-jsonl
  description: "Read 100k rows from PostgreSQL and write to JSONL"
  tags: [postgres, jsonl, smoke]

services:
  - image: images/lab-postgres-15
    alias: pg
    readiness:
      tcp: 5432
      timeout_seconds: 300
      postgres:
        dbname: lab
        user: lab
        password: lab
        probe_query: "SELECT 1 FROM spacecraft_telemetry_events LIMIT 1"

# `loadsmith:` is optional — omitted here, so the lab pulls the canonical image
# ghcr.io/loadsmith-el/loadsmith (--tag picks the version, else :slim).

pipeline:
  file: pipeline.yaml

expect:
  status: success
  rows_read: 100000
  rows_written: 100000
  output:
    file: events.jsonl      # under the /output mount (pipeline writes /output/events.jsonl)
    row_count: 100000
```
