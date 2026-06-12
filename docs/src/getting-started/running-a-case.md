# Running a Case

## List available cases

```bash
./target/debug/loadsmith-lab list
```

Output (cases are listed as `<origin>/<name>`):
```
catalog/postgres-to-jsonl
```

## Run a single case

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
```

## Run all cases

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --all
```

## What happens during a run

When you run `--select catalog/postgres-to-jsonl`, the lab:

1. **Finds the case** — resolves `catalog/postgres-to-jsonl` through the registered
   origins (a live local origin, or an installed copy) to its `case.yaml`.
2. **Creates a Docker network** named `ls-lab-<uuid>`.
3. **Resolves the service image** — the case references it as `images/lab-postgres-15`;
   the lab resolves that origin's build context and the local tag
   `loadsmith-lab/images/lab-postgres-15:local`:
   - If it's in the local Docker cache: uses it immediately.
   - If not: tries to pull from a registry, then builds from the resolved `Dockerfile`.
4. **Starts the Postgres container** on the network with hostname `pg`.
5. **Waits for TCP port 5432** to be open on the container.
6. **Waits for the Postgres probe** — runs `SELECT 1 FROM spacecraft_telemetry_events LIMIT 1`
   until it returns a row. This confirms that the `COPY` of 100k rows has committed.
7. **Resolves the loadsmith image** — with `--loadsmith <path>`, builds/wraps a
   local core; otherwise pulls the published image. Then prepares the plugin dir
   (the cached canonical set + any `--plugin` overlays), mounted at `/plugins`.
8. **Runs loadsmith in a container** on the same network, mounting the pipeline at
   `/case/pipeline.yaml`, an output dir at `/output`, and the plugin dir, with
   `run /case/pipeline.yaml --plugin-dir /plugins --log-level <level>`. The
   pipeline reaches Postgres by its alias `pg` — no rewriting.
9. **Streams** the container's output through the gutter live.
10. **Validates** exit code, row counts from the summary, and output file line count
    (read from the host side of the `/output` mount).
11. **Tears down** the containers and the Docker network.

## Forwarding flags to loadsmith

### Log level

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl --log-level debug
```

The `--log-level` flag is passed to loadsmith inside the container as
`--log-level debug`. At debug level, the full protocol handshake appears in
the framed output section of the report.

### Disabling color

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl --no-color
```

`--no-color` disables ANSI codes in the lab's own output and propagates to
loadsmith via the `NO_COLOR=1` environment variable.

## First run: image building

The first time you run `catalog/postgres-to-jsonl`, the image
`loadsmith-lab/images/lab-postgres-15:local` does not exist in your local Docker cache.
The lab builds it automatically from the `images/lab-postgres-15` build context. This
takes 2–3 minutes. After the build, the image is cached locally. Every subsequent
run (and every other case that uses the same image) starts in seconds.

To pre-build all available images without running any case:

```bash
./target/debug/loadsmith-lab build --all
```

## Running with a custom cases directory

The `--cases-dir` escape hatch resolves cases directly from a directory of
`<name>/case.yaml` subdirs (bare names, bypassing origins) — handy for CI:

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --all --cases-dir /path/to/my/cases
```
