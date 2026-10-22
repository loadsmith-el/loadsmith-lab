# case.yaml Full Schema

Complete field reference for `case.yaml` files.

```yaml
# ─── Case metadata ────────────────────────────────────────────────────────────
case:
  name: string           # required — display name in the result line; --select and
                         #   list use <origin>/<directory-name> instead (keep the
                         #   two equal by convention)
  description: string    # optional — shown in the case header during a run
  tags:                  # optional — `requires-local-plugin` skips the case without --plugin
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

  exit_code: integer     # optional — exact loadsmith exit code (0, 1, 2, 3, 4,
                         #   5, 130, 143 — see the engine's exit-code table),
                         #   e.g. 3 to assert a configuration error

  rows_read: integer     # optional — exact `rows_read` of the run summary
  rows_written: integer  # optional — exact `rows_written` of the run summary

  output:                # optional — validate an output file
    file: string         # required if output is declared — path relative to the
                         #   run's output dir (what the pipeline writes under
                         #   /output); an absolute path is a host path
    row_count: integer   # optional — expected number of lines in the file

  logs:                  # optional — assert on the loadsmith container's output
    not_contains: [string]  # optional — none may appear in stdout, stderr or
                            #   the JSON run summary (secrets: never printed)
    contains: [string]      # optional — each must appear in at least one of them
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
| `expect.logs.contains` | `[]` |
| `expect.logs.not_contains` | `[]` |

## Validation behavior

- The outcome is read from loadsmith's JSON run summary (`--summary-file`,
  schema `version: 1`). With an engine that predates the flag (the published
  `v0.2.0` images) the lab falls back to the summary box on stdout — see
  [How the lab reads the run](../getting-started/understanding-output.md#how-the-lab-reads-the-run).
- `expect.status: success` requires **both** a zero exit code and, when the
  summary reports one, status `success` (older cores exited 0 even when the
  summary said `error`). `status: error` passes when either signals failure.
- `expect.exit_code` compares the container's exit code exactly. It's
  independent of `status`, so a case that expects a configuration error writes
  `status: error` **and** `exit_code: 3`. Older engines use fewer codes (most
  failures exit `1`), so pin `--tag` or `--loadsmith` for such a case.
- `expect.rows_read` and `expect.rows_written` are the summary's `rows_read` /
  `rows_written`. On the text fallback they are parsed from the summary box
  (case-insensitive, comma-formatted numbers: `100,000` → `100000`).
- `expect.output.row_count` is the number of lines in the file (a final
  unterminated line counts).
  For JSONL output, this equals the number of JSON records.
- A missing `expect.exit_code`, `expect.rows_read` or `expect.rows_written`
  means "don't assert this value" — the case can still pass if the other assertions hold.
- If `expect.output.file` does not exist after the run, the case fails.
- `expect.logs` checks the loadsmith container's captured **stdout**, its
  **stderr** and, when the engine wrote one, the **JSON run summary** file —
  so it works on the text fallback too (there it covers stdout and stderr).
  ANSI escapes are stripped before matching; matching is a plain,
  case-sensitive substring test. Unknown keys under `logs:` (e.g. a misspelt
  `not_contain`) are rejected, and an empty needle fails the case.
- `expect.logs.not_contains` needles are treated as secrets. A failure names
  the needle by a short prefix and its length plus every stream it reached —
  `logs.not_contains: needle "s3cr…" (17 chars) found in stderr, summary file`
  — and the lab masks the needles in the live gutter and in every failure line
  (e.g. a `cause:` that carries the leak) as `[redacted "s3cr…" (17 chars)]`.
  The raw output still lands in `loadsmith.stdout` in the run's temporary
  output dir, which is deleted once the case is validated.
- `expect.logs.contains` needles are not secret: a miss prints the needle and
  the streams searched.
