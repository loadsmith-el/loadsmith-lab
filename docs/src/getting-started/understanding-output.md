# Understanding the Output

## Banner

The first line printed is the lab banner:

```
loadsmith-lab v0.1.0   local loadsmith
```

It shows the lab version and the image source (`local loadsmith` when
`--loadsmith` runs a local core, or `remote image` when using a published image).

## Case header

Before each case runs, a header is printed:

```
▶ postgres-to-jsonl
  "Read 100k rows from PostgreSQL and write to JSONL"
```

The description comes from `case.description` in `case.yaml`. If no description
is set, only the case name is shown.

## Prep lines

Setup steps are printed as dimmed lines before the loadsmith output:

```
  image loadsmith-lab/images/postgres-15:local already in cache
  waiting for postgres at 172.18.0.2:5432...
  postgres ready
```

If the image was built (not cached), the build time appears:

```
  image loadsmith-lab/images/postgres-15:local built (134.2s)
```

## Framed loadsmith output

The entire stdout of the loadsmith binary is shown inside a `│` gutter:

```
  │ Loadsmith v0.1.0  ·  postgres → jsonl
  │
  │   batch   1    2,000 rows
  │   batch   2    4,000 rows
  │   batch   4    8,000 rows
  │   batch   8   16,000 rows
  │   batch  16   32,000 rows
  │   batch  32   64,000 rows
  │   batch  50  100,000 rows
  │
  │ ─────────────────────────────────────────────────────
  │ Pipeline:     postgres-to-jsonl-smoke
  │ Route:        loadsmith-source-postgres → loadsmith-destination-jsonl
  │ Status:       success
  │ Rows read:    100,000
  │ Rows written: 100,000
  │ Batches:      50
  │ Duration:     0:01:14
  │ Throughput:   1,351 rows/s
  │ ─────────────────────────────────────────────────────
```

The framing makes it visually clear which output belongs to loadsmith and which
belongs to the lab itself. The progress lines with doubling intervals (batch 1,
2, 4, 8, 16, 32…) are loadsmith's own progress reporting.

## Verdict

After the framed output, the case verdict is printed:

**Pass:**
```
  ✓ passed   100,000 rows read · 100,000 written   74.2s
```

**Fail:**
```
  ✗ failed   2.1s
    → expected rows_read=100000, got 99998
    → output file events.jsonl: expected 100000 lines, got 99998
```

The verdict line includes:
- `✓`/`✗` icon (green/red with color enabled)
- `passed`/`failed`
- Rows read and written with thousands separators (when available)
- Wall-clock duration of the entire case (including service startup)

Failure reasons are printed as indented `→` lines underneath.

## Summary

After all cases finish:

```
────────────────────────────────────────────────────────
1 passed, 0 failed
```

The exit code of `loadsmith-lab run` reflects the summary:
- `0` — all cases passed
- `1` — one or more cases failed

## With `--no-color`

When `--no-color` is set (or `NO_COLOR` is in the environment), all ANSI escape
codes are stripped. The `│` gutter, `✓`/`✗` icons, and structural characters
remain — only the color codes are removed. This makes the output safe for log
files and CI environments where escape codes appear as garbage.

## At `--log-level debug`

The loadsmith protocol handshake appears inside the framed output:

```
  │ DEBUG loadsmith_core::lifecycle: → handshake (source)
  │ DEBUG loadsmith_core::lifecycle: ← handshake_ack name=loadsmith-source-postgres version=0.1.0
  │ DEBUG loadsmith_core::lifecycle: → set_protocol_version version=1
  │ ...
  │
  │ Loadsmith v0.1.0  ·  postgres → jsonl
  │ ...
```

loadsmith's stderr is merged into the framed output at debug and trace levels so
all relevant information is visible in context.
