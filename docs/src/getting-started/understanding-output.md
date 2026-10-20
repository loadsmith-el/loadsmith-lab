# Understanding the Output

## Banner

The first line printed is the lab banner:

```
loadsmith-lab 0.1.0  ·  local loadsmith
```

It shows the lab version and the image source (`local loadsmith` when
`--loadsmith` runs a local core, or `remote image` when using a published image).

## Case header

Before each case runs, a header is printed:

```
▶ catalog/postgres-to-jsonl  Reads 100k rows from PostgreSQL and writes them to JSONL
```

The name is the qualified `<origin>/<name>` you selected; the dimmed description
after it comes from `case.description` in `case.yaml`. If no description is set,
only the name is shown.

## Prep lines

Setup steps are printed as dimmed `·` lines before the loadsmith output. For
each service with a `readiness:` block, the lab names the service alias and the
container address it probes, then how long readiness took:

```
  · waiting for pg at 172.18.0.2:5432…
  · pg ready (2.0s)
```

A cached service image prints nothing. If the image had to be built, the build
time appears first:

```
  · built images/lab-postgres-15 in 134.2s
```

## Framed loadsmith output

loadsmith's combined stdout and stderr is shown inside a `│` gutter, at every
log level:

```
  │ loadsmith 0.2.0
  │ postgres → jsonl
  │
  │ 2026-10-16T12:00:03.112408Z  INFO loadsmith_core::reporter: schema negotiated — 34 columns
  │ ...
  │ 2026-10-16T12:00:04.201733Z  INFO loadsmith_core::reporter: 2,000 rows · 1 batch
  │ 2026-10-16T12:00:04.233019Z  INFO loadsmith_core::reporter: 4,000 rows · 2 batches
  │ 2026-10-16T12:00:04.298516Z  INFO loadsmith_core::reporter: 8,000 rows · 4 batches
  │ ...
  │ 2026-10-16T12:01:17.950274Z  INFO loadsmith_core::reporter: 100,000 rows · 50 batches
  │
  │ ───────────────────────────────────────────
  │ Pipeline:     postgres-to-jsonl-smoke
  │ Route:        loadsmith-source-postgres → loadsmith-destination-jsonl
  │ Status:       success
  │ Rows read:    100,000
  │ Rows written: 100,000
  │ Batches:      50
  │ Duration:     00:01:14.012
  │ Throughput:   1,351 rows/s
  │ ───────────────────────────────────────────
```

The framing makes it visually clear which output belongs to loadsmith and which
belongs to the lab itself. The header (`loadsmith <version>`, then the route)
and the summary box come from loadsmith's stdout. The schema and progress lines
are `tracing` records on its stderr, so each carries a timestamp, level and
target. Progress is reported at doubling batch intervals (1, 2, 4, 8, 16…) as
`<rows> rows · <n> batch(es)`.

## Verdict

After the framed output, the case verdict is printed:

**Pass:**
```
  ✓ postgres-to-jsonl   100,000 read · 100,000 written   74.2s
```

**Fail:**
```
  ✗ postgres-to-jsonl   2.1s
    → rows_read: expected 100000, got 99998
    → row_count: expected 100000, got 99998 in /tmp/ls-lab-…/events.jsonl
```

The verdict line includes:
- `✓`/`✗` icon (green/red with color enabled)
- The case name (`case.name` from `case.yaml`)
- On a pass, rows read and written with thousands separators (when available),
  as `N read · M written`
- Wall-clock duration of the entire case (including service startup)

Failure reasons are printed as indented `→` lines underneath. The messages are:

| Check | Message |
|-------|---------|
| `expect.status` | `status: expected 'success', got exit code N (summary status: …)` — `success` needs both exit code 0 and the summary's `Status: success` |
| `expect.rows_read` / `rows_written` | `rows_read: expected N, got M` (`got none` if the summary had no count) |
| `expect.output.file` | `output file not found: <path>` |
| `expect.output.row_count` | `row_count: expected N, got M in <path>`, or `cannot count rows in <path>: <error>` |

A case that fails before validation (for example, a service never became ready)
shows the error itself as the `→` line, under the qualified `<origin>/<name>` and
with a `0.0s` duration.

## Summary

After all cases finish:

```
──────────────────────────────────────────────────
1 passed, 0 failed
```

The exit code of `loadsmith-lab run` reflects the summary:
- `0` — all cases passed
- `1` — one or more cases failed

## Bundle output

`loadsmith-lab bundle run` reuses the case output above and adds its own lines:

```
■ parquet-destination  Validates the parquet destination in single-file and chunked modes
  · bundle image loadsmith-lab-bundle-parquet-destination:local ready (1.3s)

▶ catalog/postgres-to-parquet-single
  · waiting for pg at 172.18.0.2:5432…
  · pg ready (2.0s)
  │ …
  · validate: /scripts/check_single_file.py
  │ …
  · cleanup: /scripts/cleanup.sh
  ✓ postgres-to-parquet-single   100,000 read · 100,000 written   9.8s

──────────────────────────────────────────────────
parquet-destination — 2 passed, 0 failed
```

- `■ <bundle>` heads each bundle, with its description.
- `·` lines mark each hook phase (`setup:`, `validate:`, `cleanup:` plus the
  script path); the hook's own output streams in the `│` gutter.
- Around the case verdict, an entry can add `✗ setup: <reason>` (printed
  before it; the case is then skipped), `✗ validate: <reason>` (after it; both
  fail the entry) or `! cleanup: <reason>` (after it; a warning only).
- Each bundle ends with `<bundle> — N passed, M failed`.

## With `--no-color`

When `--no-color` is set (or `NO_COLOR` is in the environment), all ANSI escape
codes are stripped. The `│` gutter, `✓`/`✗` icons, and structural characters
remain — only the color codes are removed. This makes the output safe for log
files and CI environments where escape codes appear as garbage.

## At `--log-level debug`

The loadsmith protocol handshake appears inside the framed output:

```
  │ 2026-10-16T12:00:02.871140Z DEBUG loadsmith_core::lifecycle: → handshake plugin=source
  │ 2026-10-16T12:00:02.874502Z DEBUG loadsmith_core::lifecycle: ← handshake_ack plugin=source name=loadsmith-source-postgres version=0.2.1 supported=[1]
  │ 2026-10-16T12:00:02.874631Z DEBUG loadsmith_core::lifecycle: → set_protocol_version plugin=source protocol_version=1
  │ ...
  │
  │ loadsmith 0.2.0
  │ postgres → jsonl
  │ ...
```

The gutter always carries loadsmith's stderr as well as its stdout; a lower log
level just lets more `tracing` records through. `--log-level` also sets the lab's
own log filter.
