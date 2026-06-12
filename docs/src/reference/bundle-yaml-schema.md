# bundle.yaml Full Schema

Complete field reference for `bundle.yaml` files. See
[Writing Bundles](../writing-bundles/bundle-yaml.md) for the walkthrough.

```yaml
# ─── Bundle metadata ──────────────────────────────────────────────────────────
bundle:
  name: string           # required — unique identifier used in --select and reports
  description: string    # optional — shown in the bundle header during a run

# ─── Case sequence ────────────────────────────────────────────────────────────
cases:
  - case: string         # required — an existing case as <origin>/<name> (e.g. catalog/my-case)
                         #   (the case is run unchanged via the normal runner)

    setup: string        # optional — script path INSIDE the bundle image, run
                         #   before the case. No output dir exists yet.

    validate: string     # optional — script path INSIDE the bundle image, run
                         #   after the case (only if it passed). Gets the run's
                         #   output dir at /output (also argv[1]).

    cleanup: string      # optional — script path INSIDE the bundle image, always
                         #   run at the end of the entry. Gets /output like
                         #   validate. A failure here is a warning, not a fail.
```

## Companion files

A bundle directory must also contain:

| File | Purpose |
|---|---|
| `Dockerfile` | Builds `loadsmith-lab-bundle-<name>:local`, the image hooks run in. Bake in any interpreter/deps the hooks need and `COPY scripts/ /scripts/`. |
| `scripts/` | The setup/validate/cleanup scripts, made executable in the image. |

## Hook environment & arguments

| Variable / arg | `setup` | `validate` / `cleanup` |
|---|---|---|
| `LOADSMITH_LAB_BUNDLE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_CASE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_OUTPUT_DIR` (env) | — | `/output` |
| output dir bind mount | — | host run dir → `/output` |
| `argv[1]` | — | `/output` |

A hook's exit code is its verdict: `0` = pass, non-zero = fail.

## Defaults

| Field | Default |
|---|---|
| `bundle.description` | none |
| `cases[].setup` | none (skipped) |
| `cases[].validate` | none (skipped) |
| `cases[].cleanup` | none (skipped) |

## Validation behavior

- An **entry passes** only if `setup` (if any) exited 0, the case's own `expect`
  block passed, and `validate` (if any) exited 0.
- A failing `setup` skips the case and `validate`; `cleanup` still runs.
- A failing case skips `validate`; `cleanup` still runs.
- `cleanup` **always** runs; a non-zero exit is reported as a warning and does
  **not** fail the entry.
- The bundle runs **every** entry regardless of earlier failures, then prints an
  aggregated `N passed, M failed` summary. The process exit code is `0` only if
  every entry passed.
