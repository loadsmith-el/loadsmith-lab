# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository. It is **operating instructions only** — for what
loadsmith-lab is, its architecture, and how cases work, read
[README.md](README.md) and the docs under [`docs/src`](docs/src), or the
source itself. Don't guess at "why" — go read it.

## Conventions

- **English only.** All artifacts committed to this repo — docs, code comments,
  commit messages, identifiers — must be in English, even when the user writes
  in Portuguese.
- **Keep docs in sync.** Whenever you change behavior, commands, architecture,
  or what's shipped, check whether `README.md`, the docs under
  [`docs/src`](docs/src), and [`ROADMAP.md`](ROADMAP.md) need updating too —
  "doc" means all three, not just one. Docs that drift from the code are worse
  than no docs.
- **Multi-arch.** Loadsmith images are published for both `linux/amd64` and
  `linux/arm64` (AWS Graviton support — see `../loadsmith/CLAUDE.md`). Never
  hardcode a `platform` in the lab's bollard calls
  ([`client.rs`](crates/loadsmith-lab-docker/src/client.rs),
  [`image.rs`](crates/loadsmith-lab-runner/src/image.rs)) — letting Docker
  resolve the host's native architecture is what lets the same lab run
  unmodified on an amd64 dev box or an arm64/Graviton host. New service images
  (`loadsmith-lab-images/images/<name>/Dockerfile`) should be based on images
  that publish official `arm64` variants too (most do — `postgres`, `debian`, …).

## Origins: engine + catalog + images

This repo is the **engine only** — it ships no cases/bundles/images. Content
lives in two sibling repos resolved as **origins**, everything addressed as
`<origin>/<name>`:

- `loadsmith-lab-catalog` — `cases/<name>/` + `bundles/<name>/` + a root
  `loadsmith-lab.toml` manifest (name → description per category).
- `loadsmith-lab-images` — `images/<name>/` build contexts (Dockerfile + init)
  + a root `loadsmith-lab.toml` manifest. No committed seed data: each image's
  Dockerfile **generates the canonical CSV at build time** in a build stage that
  clones `loadsmith-lab-canonical-data` (pinned tag) and runs `generate.py`,
  then `COPY --from=data` bakes it in. (Tradeoff: a fresh image build needs
  network; runtime and cached builds stay offline.)
- `loadsmith-lab-canonical-data` — the lone generator (`generate.py`,
  stdlib-only, deterministic seed=42) + the schema contract. Never commits a
  CSV — the data is a pure function of the generator, reproduced at build time.

An origin is **remote** (a git repo, registered then `install`ed into the XDG
workdir) or **local** (a path read live in place — the dev workflow). A case's
service image is itself an `<origin>/<name>` reference (e.g.
`image: images/postgres-15`); the runner resolves its build context from that
origin via [`origin.rs`](crates/loadsmith-lab-runner/src/origin.rs) and builds
a local tag `loadsmith-lab/<origin>/<name>:local`. See
[docs/src/architecture/overview.md](docs/src/architecture/overview.md).

- **New cases/bundles/images go in the catalog/images repos, not here** — add
  the content dir *and* a manifest entry in that repo's `loadsmith-lab.toml`.

## Commands

```bash
cargo build                                            # build the workspace

# Dev setup (once): register the sibling content repos as LOCAL origins (read live)
cargo run -p loadsmith-lab-cli -- origin local add catalog ../loadsmith-lab-catalog
cargo run -p loadsmith-lab-cli -- origin local add images  ../loadsmith-lab-images

cargo run -p loadsmith-lab-cli -- list                 # list available cases (<origin>/<name>)
cargo run -p loadsmith-lab-cli -- run --select catalog/postgres-to-jsonl
cargo run -p loadsmith-lab-cli -- run --all
cargo run -p loadsmith-lab-cli -- run --all --loadsmith ../loadsmith            # build the core from source (a project dir or a binary)
cargo run -p loadsmith-lab-cli -- run --select catalog/postgres-to-jsonl --loadsmith ../loadsmith --plugin ../loadsmith-canonical-plugins/jsonl   # override a plugin with a local build
cargo run -p loadsmith-lab-cli -- build --all          # build all available service images
cargo run -p loadsmith-lab-cli -- build --select images/postgres-15
cargo run -p loadsmith-lab-cli -- --log-level debug run --select catalog/postgres-to-jsonl   # verbose; logs go to stderr
```

The canonical seed CSV is regenerated at image-build time from
`loadsmith-lab-canonical-data` (there is no `generate` lab command). To change
the dataset, edit `generate.py` there, bump its tag, and point images at the new
`DATA_REF`.

Requires Docker. See [README.md § Adding a service or case](README.md#adding-a-service-or-case)
before adding a new case or image.

## Hard rules — read before adding a case

- **Volume/scale cases (anything that inflates row counts via `CROSS JOIN` /
  `generate_series`) MUST use `destination.type: "null"`**
  (`loadsmith-destination-null`, quoted in YAML) and assert only
  `rows_read`/`rows_written` — no `output:` block. A multi-million-row JSONL is
  gigabytes, and the output dir defaults to the system temp dir (often a tmpfs
  that can't hold it). Name them `<service>-to-null-<N>`
  (e.g. `postgres-to-null-15M`). The 100k smoke case (`postgres-to-jsonl`) is
  the one that validates real content/type round-trips — don't conflate the two
  purposes.
