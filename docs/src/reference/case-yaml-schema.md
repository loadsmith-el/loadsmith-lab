# case.yaml Full Schema

Complete field reference for `case.yaml` files.

```yaml
# ─── Case metadata ────────────────────────────────────────────────────────────
case:
  name: string           # required — unique identifier used in --select and reports
  description: string    # optional — shown in the case header during a run
  tags:                  # optional — reserved for future filtering
    - string

# ─── Services ─────────────────────────────────────────────────────────────────
services:
  - image: string        # required — Docker image name
                         #   Custom lab images: loadsmith-lab-{service}:{tag}
                         #   Public images: redis:7, etc.

    alias: string        # required — hostname in the Docker network
                         #   Use this in pipeline.yaml as the service host

    readiness:           # optional — wait conditions before loadsmith runs
      tcp: integer       # required if readiness is declared — port to poll
      timeout_seconds: integer  # optional — total timeout for all probes, default: 60

      postgres:          # optional — Postgres query-level probe (run after TCP is open)
        dbname: string   # required
        user: string     # required
        password: string # required
        probe_query: string  # optional — SQL that must return ≥1 row
                             # default: "SELECT 1"

    env:                 # optional — environment variables for the service container
      - "KEY=VALUE"

    docker_args:         # optional — extra arguments passed verbatim to docker run
      - string

# ─── Loadsmith invocation ─────────────────────────────────────────────────────
loadsmith:               # optional block (whole thing) — omit unless overriding
  image: string          # optional — full image ref override. Normally omitted:
                         #   the lab pulls ghcr.io/loadsmith-el/loadsmith (--tag
                         #   picks the version, else :slim). Ignored when
                         #   --loadsmith is given (builds/wraps a local core).

  volumes:               # optional — EXTRA bind mounts (the output dir is always
    - host: string       #   mounted at /output automatically; don't remap /output)
      container: string  #   host:container absolute paths

  env:                   # optional — environment variables for loadsmith
    - "KEY=VALUE"

  docker_args:           # optional — extra arguments for docker run
    - string

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
    file: string         # required if output is declared — absolute path to the file
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
| `loadsmith.volumes` | `[]` |
| `loadsmith.env` | `[]` |
| `loadsmith.docker_args` | `[]` |

## Validation behavior

- `expect.rows_read` and `expect.rows_written` are parsed from the loadsmith summary
  printed to stdout. The parser is case-insensitive and handles comma-formatted
  numbers (`100,000` → `100000`).
- `expect.output.row_count` is the number of newline-terminated lines in the file.
  For JSONL output, this equals the number of JSON records.
- A missing `expect.rows_read` or `expect.rows_written` means "don't assert this
  value" — the case can still pass if the other assertions hold.
- If `expect.output.file` does not exist after the run, the case fails.
