# Run Modes

**Loadsmith always runs inside a Docker container** in the lab. There is a single
execution path: loadsmith runs on the case's Docker bridge network and reaches
services by their alias hostnames (`pg`, `redis`, …) — no host-port publishing, no
hostname rewriting.

`--loadsmith` and `--plugin` don't change *how* loadsmith runs; they only select
*where the core and plugins come from*.

## `--loadsmith <path>` — run a local core

Point `--loadsmith` at a **project dir** or a **prebuilt binary**:

- **project dir** → the lab builds a `loadsmith:local` image from its Dockerfile,
  compiling hermetically inside Docker (multi-stage, cargo-chef-cached).
- **binary** → the lab wraps it in a minimal `debian:bookworm-slim` image (the
  binary must be runnable there: a v0.2.0+ release binary is static musl and
  always works; a host glibc build needs glibc ≤ bookworm's).

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
```

The first project build compiles loadsmith inside Docker (slower; cached
afterward via cargo-chef's dependency layer). The build is hermetic — it compiles
a static musl binary inside `rust:1-alpine`, and the lab builds the Dockerfile's
default `slim` target (`debian:bookworm-slim`), so it is correct regardless of the
host's glibc; the host's `target/` is never copied in.

Without `--loadsmith`, the lab pulls the **canonical published image**
`ghcr.io/loadsmith-el/loadsmith` — like the plugin index and the content origins,
the package is fixed. The image is chosen in this order: `--tag <tag>` >
a case's `loadsmith.image` (a full ref, used only when no `--tag` is given) >
the rolling `:slim` variant.
Resolved via local cache → registry pull; if not pullable, the run fails with a
clear error.

```bash
./target/debug/loadsmith-lab run --select catalog/postgres-to-jsonl                    # ghcr.io/loadsmith-el/loadsmith:slim
./target/debug/loadsmith-lab run --tag v0.2.0-slim --select catalog/postgres-to-jsonl  # pin a version
./target/debug/loadsmith-lab run --tag static --select catalog/postgres-to-jsonl       # another image flavor
```

`--tag` takes any published tag, so it also selects the image flavor (`static`,
`static-debug`, `slim`, `bundled`, rolling or versioned like `v0.2.0-static`).
The lab mounts its plugin cache at `/plugins` in every flavor; the cached
plugins are static binaries, so they run in the distroless ones too.

## Plugins — a mounted cache, with `--plugin` overrides

The default image (`:slim`) is core-only. With any flavor, including `bundled`,
the lab keeps a host plugin cache (`~/.cache/loadsmith-lab/plugins`), populated
once by running `loadsmith plugin install --all` (the canonical set) in a
throwaway container, and **mounts it into every run** at `/plugins`
(`--plugin-dir /plugins`), which wins over plugins on `PATH`.

The cache is filled only when the directory is empty and is never updated. To
pick up new plugin releases, delete it; the next run reinstalls the canonical set:

```bash
rm -rf ~/.cache/loadsmith-lab/plugins
```

To test a plugin you're developing, override it with `--plugin <path>` (repeatable):

- **binary** → overlaid directly over the cache.
- **plugin crate** (e.g. `../loadsmith-canonical-plugins/jsonl`) → built in a
  `rust:1-alpine` container as a static musl binary (the same build a release
  ships, so it runs in any loadsmith image), then overlaid.
- **workspace root** (e.g. `../loadsmith-canonical-plugins`) → builds every member
  and overlays them all.

Like the `--loadsmith` core image (built or wrapped once per invocation, not per
case), overrides are resolved **once per lab invocation**: every case of a `run --all`
or bundle shares the same overlay (a temp dir), so a workspace is compiled once,
not once per case. The build runs as the host user with a persistent
`CARGO_HOME` (`~/.cache/loadsmith-lab/cargo-home`, caches the crate registry);
its temp target dir is deleted as soon as the binaries are copied into the
overlay, and the overlay is deleted at the end of the run.

```bash
# dev core + a locally-built jsonl, everything else from the cache:
./target/debug/loadsmith-lab run --loadsmith ../loadsmith \
  --plugin ../loadsmith-canonical-plugins/jsonl --select catalog/postgres-to-jsonl
```

## How loadsmith runs in the container

The lab:

1. Resolves the loadsmith image (`--loadsmith` build/wrap, or pull a published one).
2. Prepares the plugin dir (cache + any `--plugin` overlays) and mounts it at `/plugins`.
3. Runs the image on the case's Docker network so it reaches services by alias.
4. Mounts the resolved `pipeline.yaml` at `/case/pipeline.yaml` and an output dir at `/output`.
5. Drives the container with
   `loadsmith run /case/pipeline.yaml --plugin-dir /plugins --log-level <level>`
   (the image's `ENTRYPOINT` is `loadsmith`), propagating `NO_COLOR` when needed.
   When the image's `run --help` lists `--summary-file` (checked once per image),
   it also passes `--summary-file /lab-summary/summary.json` with a scratch dir
   mounted there, and builds the verdict from that JSON run summary; an older
   engine (the published `v0.2.0` images) gets no flag and its summary box is
   scraped instead.
6. Streams the container's combined stdout+stderr through the report gutter in real
   time, and validates the output file under the host side of the `/output` mount.
   The container runs as the host user, so outputs aren't root-owned; the output
   dir is deleted once the case is validated (after the hooks, for bundles).
