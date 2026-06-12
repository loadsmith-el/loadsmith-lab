# case.yaml

Each case lives in its own directory under `cases/` in the
`loadsmith-lab-canonical-catalog` repo (the "catalog" origin), addressed as
`catalog/<name>`. A case directory contains exactly two files:

```
loadsmith-lab-canonical-catalog/cases/my-case/
  case.yaml      ← what services to start and what to assert
  pipeline.yaml  ← the loadsmith pipeline to run
```

Add a matching entry under `[cases]` in `loadsmith-lab-canonical-catalog/loadsmith-lab.toml`
so the case appears in the origin's manifest.

## Full structure

```yaml
case:
  name: string           # required
  description: string    # optional
  tags: [string]         # optional

services:
  - image: string        # required — Docker image name
    alias: string        # required — hostname in the Docker network
    readiness:           # optional
      tcp: integer       # port to wait for
      timeout_seconds: integer  # default: 60
      postgres:          # optional — additional Postgres-level probe
        dbname: string
        user: string
        password: string
        probe_query: string  # optional — default: "SELECT 1"
    env: [string]        # optional — "KEY=VALUE" strings
    docker_args: [string] # optional — extra arguments to docker run

loadsmith:
  image: string          # "local" or a Docker image name
  volumes:               # optional
    - host: string
      container: string
  env: [string]          # optional
  docker_args: [string]  # optional

pipeline:
  file: string           # path to pipeline.yaml, relative to the case directory

expect:
  status: string         # "success" or "error"
  rows_read: integer     # optional — exact rows_read asserted
  rows_written: integer  # optional — exact rows_written asserted
  output:                # optional
    file: string         # path to output file
    row_count: integer   # optional — number of lines expected in the file
```

## Section reference

### `case`

Metadata about the case.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Unique identifier. Used in `--select`, in the report, and in `loadsmith-lab list` |
| `description` | string | no | Shown in the case header when running |
| `tags` | list of strings | no | Arbitrary tags. Unused in v0.1.0 — reserved for future `--tag` filtering |

### `services`

A list of Docker services to start before running loadsmith. Each service is
started in order and must pass its readiness check before the next one starts.

| Field | Type | Required | Description |
|---|---|---|---|
| `image` | string | yes | Docker image name. Lab images follow `loadsmith-lab-{service}:{tag}` |
| `alias` | string | yes | Hostname assigned to the container in the Docker network. Use this in `pipeline.yaml` |
| `readiness.tcp` | integer | no | Port number to wait for. Polled every 500 ms |
| `readiness.timeout_seconds` | integer | no | Total timeout for all readiness checks. Default: 60 |
| `readiness.postgres` | object | no | Additional Postgres-level readiness probe |
| `env` | list of strings | no | Environment variables passed to the container (`"KEY=VALUE"`) |
| `docker_args` | list of strings | no | Extra arguments forwarded verbatim to docker run (escape hatch) |

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

### `loadsmith`

How to run loadsmith for this case.

| Field | Type | Required | Description |
|---|---|---|---|
| `image` | string | yes | Published image for the default path (e.g. `loadsmith:latest`) |
| `volumes` | list | no | Extra bind mounts. Each entry has `host` and `container` paths |
| `env` | list of strings | no | Environment variables for the loadsmith process |
| `docker_args` | list of strings | no | Extra arguments for docker run |

Loadsmith always runs in a container. The `image` field names the published image
used by the default path; with `--loadsmith <path>` the lab builds/wraps a local
core and ignores `image`. The output directory is always mounted at
`/output` automatically, so `volumes` is only for *additional* mounts (don't remap
`/output`).

### `pipeline`

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | Path to the pipeline YAML, relative to the case directory |

### `expect`

Assertions validated after the run. A case fails if any assertion does not hold.

| Field | Type | Required | Description |
|---|---|---|---|
| `status` | string | yes | Expected outcome. `"success"` or `"error"` |
| `rows_read` | integer | no | Expected `Rows read` from the loadsmith summary |
| `rows_written` | integer | no | Expected `Rows written` from the loadsmith summary |
| `output.file` | string | no | Output file to check. Relative paths resolve under the host side of the `/output` mount |
| `output.row_count` | integer | no | Expected number of lines in the output file |

Row counts are parsed from the loadsmith summary box printed to stdout. The
parser is case-insensitive and handles comma-formatted numbers (`100,000`).

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

loadsmith:
  image: loadsmith:latest   # used by the default path; ignored when --loadsmith is given

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
