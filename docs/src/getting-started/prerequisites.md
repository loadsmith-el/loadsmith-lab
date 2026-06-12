# Prerequisites

## Required

- **Docker** — the Docker daemon must be running. Verify with `docker info`.
- **Rust toolchain** (stable) — install via [rustup.rs](https://rustup.rs).
- **A local loadsmith / plugins checkout** (optional) — if you'll run with
  `--loadsmith <path>` / `--plugin <path>`. A project dir is built inside Docker
  (binaries are never copied from the host), so no host pre-build is required.

## Directory layout

loadsmith-lab expects to live alongside the loadsmith repository and its content
repos:

```
projects/
  loadsmith/                ← the main repo
  loadsmith-lab/            ← this repo (the engine, sibling directory)
  loadsmith-lab-catalog/    ← cases + bundles (the "catalog" origin)
  loadsmith-lab-images/     ← service images (the "images" origin)
```

`--loadsmith <path>` / `--plugin <path>` take any path; the sibling layout above
is just what the examples in these docs assume.

## Initial setup

**1. (optional) Build loadsmith for its unit tests:**

```bash
cd ../loadsmith && cargo build
# Not required to run the lab — `--loadsmith ../loadsmith` builds the core image
# in Docker. Handy for loadsmith's own unit tests during development.
```

**2. Build loadsmith-lab:**

```bash
cd ../loadsmith-lab
cargo build
# produces: target/debug/loadsmith-lab
```

**3. Verify Docker is running:**

```bash
docker info
```

**4. Register the content repos as local origins (read live, no install):**

```bash
./target/debug/loadsmith-lab origin local add catalog ../loadsmith-lab-catalog
./target/debug/loadsmith-lab origin local add images  ../loadsmith-lab-images
```

**5. Run a smoke test:**

```bash
./target/debug/loadsmith-lab run --loadsmith ../loadsmith --select catalog/postgres-to-jsonl
```

On the first run, the lab builds two images automatically: the `images/postgres-15`
image (tagged `loadsmith-lab/images/postgres-15:local`, seeds 100,000 rows, ~2–3 min)
and, because of `--loadsmith`, `loadsmith:local` from the source tree (cargo-chef
cooks dependencies once, then caches them). Subsequent runs reuse both cached
layers and start quickly.

> **Network note:** a *fresh* image build clones `loadsmith-lab-canonical-data` to
> generate the seed CSV, so the first build of an image needs network access.
> Cached images and every case *run* are fully offline.

## Optional: Python (for previewing test data)

You don't need Python to run cases — the seed CSV is generated inside the image
build. Python is only useful if you want to **preview** the dataset locally or
change the schema. The generator is **stdlib-only** (no `pip install`):

```bash
cd ../loadsmith-lab-canonical-data && python generate.py   # → spacecraft_telemetry_events.csv (gitignored)
```
