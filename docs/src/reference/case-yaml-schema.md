# case.yaml Full Schema

Complete field reference for `case.yaml` files.

```yaml
# ─── Case metadata ────────────────────────────────────────────────────────────
case:
  name: string           # required — display name in the result line; --select and
                         #   list use <origin>/<directory-name> instead (keep the
                         #   two equal by convention)
  description: string    # optional — shown in the case header during a run
  tags:                  # optional — reserved for future filtering
    - string

# ─── Services ─────────────────────────────────────────────────────────────────
services:                # required — may be an empty list (services: [])
  - image: string        # required — service image as <origin>/<name>
                         #   (e.g. images/lab-postgres-15); built locally as
                         #   loadsmith-lab/<origin>/<name>:local

    alias: string        # required — hostname in the Docker network
                         #   Use this in pipeline.yaml as the service host

    readiness:           # optional — wait conditions before loadsmith runs
      tcp: integer       # required if readiness is declared — port to poll
      timeout_seconds: integer  # optional — per-stage timeout, default: 60; the TCP
                                #   wait and each query probe each get the full value

      postgres:          # optional — Postgres query-level probe (run after TCP is open)
        dbname: string   # required
        user: string     # required
        password: string # required
        probe_query: string  # optional — SQL that must return ≥1 row
                             # default: "SELECT 1"

      mysql:             # optional — MySQL query-level probe (run after TCP is open)
        dbname: string   # required
        user: string     # required
        password: string # required
        probe_query: string  # optional — SQL that must return ≥1 row
                             # default: "SELECT 1"

      oracle:            # optional — Oracle query-level probe (run after TCP is open)
        service_name: string  # required — e.g. FREEPDB1
        user: string     # required
        password: string # required
        probe_query: string  # optional — SQL that must return ≥1 row
                             # default: "SELECT 1 FROM dual"

    env:                 # optional — environment variables for the service container
      - "KEY=VALUE"

    docker_args:         # optional — despite the name, each entry is used as a
      - string           #   bind-mount spec (host:container[:opts]), not a
                         #   docker run flag

# ─── Loadsmith invocation ─────────────────────────────────────────────────────
loadsmith:               # optional block (whole thing) — omit unless overriding
  image: string          # optional — full image ref override. Normally omitted:
                         #   the lab pulls ghcr.io/loadsmith-el/loadsmith (--tag
                         #   picks the version, else :slim). Ignored when
                         #   --loadsmith or --tag is given.

  volumes:               # optional — EXTRA bind mounts (the output dir is always
    - host: string       #   mounted at /output automatically; don't remap /output)
      container: string  #   host: relative to the case dir (resolved at run
                         #   time) or absolute; container: absolute

  env:                   # optional — environment variables for loadsmith
    - "KEY=VALUE"

  docker_args:           # optional — currently ignored (accepted but never
    - string             #   applied to the loadsmith container)

# ─── Pipeline ─────────────────────────────────────────────────────────────────
pipeline:
  file: string           # required — path to pipeline.yaml, relative to the case directory

# ─── Assertions ───────────────────────────────────────────────────────────────
expect:
  status: string         # required — "success" or "error"
                         #   "success" → loadsmith exited with code 0
                         #   "error"   → loadsmith exited with non-zero

  rows_read: integer     # optional — exact value of "Rows read" in the summary box
  rows_written: integer  # optional — exact value of "Rows written" in the summary box

  output:                # optional — validate an output file
    file: string         # required if output is declared — path relative to the
                         #   run's output dir (what the pipeline writes under
                         #   /output); an absolute path is a host path
    row_count: integer   # optional — expected number of lines in the file
```

## Defaults

| Field | Default |
|---|---|
| `case.tags` | `[]` |
| `services[].env` | `[]` |
| `services[].docker_args` | `[]` |
| `readiness.timeout_seconds` | `60` |
| `readiness.postgres.probe_query` | `"SELECT 1"` |
| `readiness.mysql.probe_query` | `"SELECT 1"` |
| `readiness.oracle.probe_query` | `"SELECT 1 FROM dual"` |
| `loadsmith.volumes` | `[]` |
| `loadsmith.env` | `[]` |
| `loadsmith.docker_args` | `[]` |

## Validation behavior

- `expect.status: success` requires **both** a zero exit code and, when the summary
  box is printed, `Status: success` (older cores exited 0 even when the summary
  said `error`). `status: error` passes when either signals failure.
- `expect.rows_read` and `expect.rows_written` are parsed from the loadsmith summary
  printed to stdout. The parser is case-insensitive and handles comma-formatted
  numbers (`100,000` → `100000`).
- `expect.output.row_count` is the number of lines in the file (a final
  unterminated line counts).
  For JSONL output, this equals the number of JSON records.
- A missing `expect.rows_read` or `expect.rows_written` means "don't assert this
  value" — the case can still pass if the other assertions hold.
- If `expect.output.file` does not exist after the run, the case fails.
