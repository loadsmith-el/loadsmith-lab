# bundle.yaml Full Schema

Complete field reference for `bundle.yaml` files. See
[Writing Bundles](../writing-bundles/bundle-yaml.md) for the walkthrough.

```yaml
# ─── Bundle metadata ──────────────────────────────────────────────────────────
bundle:
  name: string           # required — names the hook image
                         #   (loadsmith-lab-bundle-<name>:local), the header and
                         #   the summary; --select uses <origin>/<directory-name>
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
                         #   validate, but only if the case actually ran (not
                         #   after a setup failure or a case infra error).
                         #   A failure here is a warning, not a fail.
```

## Companion files

Besides `bundle.yaml`, a bundle directory contains:

| File | Required | Purpose |
|---|---|---|
| `Dockerfile` | always | Builds `loadsmith-lab-bundle-<name>:local`, the image hooks run in (it is built on every run, even with no hooks). Bake in any interpreter/deps the hooks need and `COPY scripts/ /scripts/`. |
| `scripts/` | only with hooks | The setup/validate/cleanup scripts, made executable in the image. A hookless bundle (e.g. `catalog/tls-spike`, whose Dockerfile is just `FROM debian:12-slim`) needs none. |

## Hook environment & arguments

| Variable / arg | `setup` | `validate` / `cleanup` |
|---|---|---|
| `LOADSMITH_LAB_BUNDLE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_CASE_NAME` (env) | ✓ | ✓ |
| `LOADSMITH_LAB_OUTPUT_DIR` (env) | — | `/output` |
| output dir bind mount | — | host run dir → `/output` |
| `argv[1]` | — | `/output` |

The `/output` mount, `LOADSMITH_LAB_OUTPUT_DIR` and `argv[1]` are provided to
`validate`/`cleanup` only when the case run produced a result (it may still have
failed its `expect`). If setup failed, the case reference didn't resolve, or the
case errored before producing a result, `cleanup` runs without any of them, so a
cleanup script must handle a missing `/output`.

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
  aggregated `<bundle> — N passed, M failed` summary. The process exit code is `0` only if
  every entry passed.
