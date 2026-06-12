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
  binary must be runnable there — linux, glibc ≤ the base's).

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
```

The first project build compiles loadsmith inside Docker (slower; cached
afterward via cargo-chef's dependency layer). The build is hermetic — it compiles
inside a Debian-based Rust image and ships on a matching Debian runtime, so it is
correct regardless of the host's glibc; the host's `target/` is never copied in.

Without `--loadsmith`, the lab uses a **published image** (`--tag <tag>` →
`loadsmith:<tag>`, otherwise `case.loadsmith.image`), resolved via local cache →
registry pull; if neither present nor pullable, the run fails with a clear error.

```bash
./target/debug/loadsmith-lab run --select catalog/postgres-to-jsonl              # case.loadsmith.image
./target/debug/loadsmith-lab run --tag 0.1.0 --select catalog/postgres-to-jsonl  # loadsmith:0.1.0
```

## Plugins — a mounted cache, with `--plugin` overrides

The loadsmith image is **slim** (core only); plugins are not bundled. The lab keeps
a host plugin cache (`~/.cache/loadsmith-lab/plugins`), populated once by running
`loadsmith plugin install --all` (the canonical set) in a throwaway container, and
**mounts it into every run** at `/plugins` (`--plugin-dir /plugins`).

To test a plugin you're developing, override it with `--plugin <path>` (repeatable):

- **binary** → overlaid directly over the cache.
- **plugin crate** (e.g. `../loadsmith-canonical-plugins/jsonl`) → built in a
  `rust:bookworm` container (glibc-matches the image), then overlaid.
- **workspace root** (e.g. `../loadsmith-canonical-plugins`) → builds every member
  and overlays them all.

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
6. Streams the container's combined stdout+stderr through the report gutter in real
   time, and validates the output file under the host side of the `/output` mount.
