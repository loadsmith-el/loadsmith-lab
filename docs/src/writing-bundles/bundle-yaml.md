# bundle.yaml

A **bundle** runs several cases as one hands-off sequence. Each case in the
sequence can be wrapped with hook scripts — `setup` → run the case →
`validate` → `cleanup` — so a whole scenario (set up, run, assert on the real
output, clean up, repeat) runs end to end with no manual steps.

A bundle never changes a case. Cases stay exactly what they are — standalone
units you can still run with `loadsmith-lab run --select <case>`. A bundle only
*chains* and *wraps* cases that already work on their own.

## Why bundles exist

A case's built-in `expect` block can only check the run status, row counts, and
the line count of a single text file. That can't prove a *binary* or
*multi-file* output is actually correct — e.g. that the parquet destination
produced one valid file in single-file mode, or N valid chunk files in split
mode. Bundles close that gap with **pluggable hook scripts** that can assert on
any output in any way (open the Parquet files with pyarrow, diff a checksum,
query a database, …).

## Layout

Each bundle lives in its own directory under `bundles/` in the
`loadsmith-lab-canonical-catalog` repo, addressed as `catalog/<name>`:

```
loadsmith-lab-canonical-catalog/bundles/my-bundle/
  bundle.yaml      ← the case sequence + hook script paths
  Dockerfile       ← builds the image the hook scripts run in
  scripts/         ← setup/validate/cleanup scripts, COPYed into the image
```

Add a matching entry under `[bundles]` in
`loadsmith-lab-canonical-catalog/loadsmith-lab.toml`.

## Structure

```yaml
bundle:
  name: string           # required — used in --select and the report
  description: string    # optional — shown in the bundle header

cases:
  - case: string         # required — an existing case as <origin>/<name> (e.g. catalog/my-case)
    setup: string        # optional — script path INSIDE the bundle image
    validate: string     # optional — script path INSIDE the bundle image
    cleanup: string      # optional — script path INSIDE the bundle image
```

Hook paths are paths **inside the bundle image** (e.g.
`/scripts/check_single_file.py`), not host paths — see below.

## Hooks run in a container, not on your host

This is the key design choice: hook scripts run **inside a container built from
the bundle's own `Dockerfile`**, exactly like loadsmith itself always runs in a
container. Everything a hook needs (a Python interpreter, pyarrow, anything
else) is baked into that image. A bundle run therefore needs **nothing
installed on your machine beyond Docker** — no host Python, no `pip install`,
no virtualenv. It runs identically on any machine with Docker.

This is *not* Docker-in-Docker. The lab orchestrates all containers from the
host via the Docker socket; a hook container is just one more container it runs.

The bundle image is rebuilt before every run, but Docker's layer cache makes
repeats fast (and picks up script edits automatically). The tag is
`loadsmith-lab-bundle-<name>:local`.

### Example Dockerfile

```dockerfile
FROM python:3.12-slim
RUN pip install --no-cache-dir pyarrow
COPY scripts/ /scripts/
RUN chmod +x /scripts/*
```

## The hook contract

Each hook is run as a one-shot container. The runner invokes the script path
directly, relying on the script's shebang + executable bit (set by the
Dockerfile) — there is no interpreter-guessing, so a hook can be Python, shell,
or anything else.

| | `setup` | `validate` / `cleanup` |
|---|---|---|
| `LOADSMITH_LAB_BUNDLE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_CASE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_OUTPUT_DIR` (env) | — | `/output` |
| output dir bind mount | — | host run dir → `/output` |
| `argv[1]` | — | `/output` |

- **`setup`** runs *before* the case, so no output exists yet — it gets the
  bundle/case names but no output dir.
- **`validate`** and **`cleanup`** run *after* the case, with the run's output
  directory bind-mounted at `/output` and also passed as the first argument.
- **Exit code** is the verdict: `0` = pass, non-zero = fail.
- stdout/stderr is streamed live into the report under the case.

Hooks do not join the case's service network (services are already gone by the
time `validate`/`cleanup` run). Hooks are for asserting on output files, not for
talking to live services.

## Execution and failure handling

For each entry, in order:

1. **setup** (if present). If it fails, the case and `validate` are skipped, but
   `cleanup` still runs.
2. **the case** (via the normal case runner). If the case's own `expect` fails,
   `validate` is skipped, but `cleanup` still runs.
3. **validate** (if present, only when the case passed).
4. **cleanup** (if present) — **always** runs. A cleanup failure is reported as
   a *warning*, not an entry failure.

An entry passes only if setup succeeded, the case passed, and validate
succeeded. The bundle **runs every entry regardless of earlier failures** and
prints an aggregated `N passed, M failed` summary at the end, so one broken
entry never hides the health of the others. The lab removes the per-run
temporary output directory after each entry's cleanup.

## Running

```bash
loadsmith-lab bundle list
loadsmith-lab bundle run --loadsmith ../loadsmith --select catalog/my-bundle
loadsmith-lab bundle run --loadsmith ../loadsmith --all
```

The shipped `catalog/parquet-destination` bundle is a complete worked example:
it validates the parquet destination in both single-file and chunked modes,
opening the produced files with pyarrow.

For the exhaustive field list, see the
[bundle.yaml schema reference](../reference/bundle-yaml-schema.md).
