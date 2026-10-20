# Docker Model

Every case run gets its own isolated Docker environment, created from scratch and
torn down completely at the end.

## Network isolation

The lab creates a Docker bridge network with a UUID-based name before starting
any container:

```
ls-lab-3f2a1c8e4b7d9f0a2e5c
```

All service containers are attached to this network, and loadsmith runs on it too,
so loadsmith reaches services by their alias hostnames. Service containers are
also *named* after their alias (e.g. `pg`), and container names are global to the
Docker daemon, so concurrent runs whose cases share a service alias are not
supported: the second one fails to create its container, and teardown's
by-alias fallback could remove the other run's container.

The network is removed after teardown, even if the run fails: the case body runs
in `run_case_inner` and its `Result` is captured, then the service containers (by
ID, then by alias) and the network are always removed before the result is
returned. This covers errors, not panics.

## Container lifecycle

For each service declared in `case.yaml`:

1. **Image resolution** — the case references its service image as an
   `<origin>/<name>` reference (e.g. `images/lab-postgres-15`); the runner resolves
   that origin's build context and derives the local tag
   `loadsmith-lab/<origin>/<name>:local`, then tries in order:
   - Local Docker cache (image already exists): instant
   - Pull from registry: `docker pull <tag>` — always fails today and falls
     through to the build, since the `:local` tag is never in a registry
   - Build from the resolved `Dockerfile`: auto-builds the first time

2. **Start** — `docker run` with:
   - Hostname set to the service alias (e.g., `--hostname pg`)
   - Attached to the case network
   - Environment variables from `env` field

3. **Readiness wait** — two levels, probed from the host against the container's
   bridge IP on the case network (the alias is only resolvable inside Docker):
   - TCP: poll `TcpStream::connect` every 500 ms until the port accepts connections
   - Query probe (optional): `postgres:` or `mysql:` (`dbname`, `user`,
     `password`, `probe_query`, default `SELECT 1`) or `oracle:` (`service_name`,
     `user`, `password`, `probe_query`, default `SELECT 1 FROM dual`) — connect
     and run the probe query every 2 s until it returns at least one row
   - Each stage gets its own `timeout_seconds` (default 60)

4. **Teardown** — `docker stop` then `docker rm --force -v`. This runs
   unconditionally after the case finishes; `-v` also removes the container's
   anonymous volumes (database images declare a `VOLUME` for their data dir, so
   without it every run would leak one).

## Leaving nothing behind

Everything the lab creates is cleaned up, even across interrupted runs:

- Every container and network carries the label `loadsmith-lab=true`. At the
  start of each `run`/`bundle run`, `DockerClient::prune_stale("1h")` removes
  *stopped* lab containers and *unused* lab networks older than an hour —
  leftovers of a run killed before its own teardown. The age guard keeps it from
  touching a concurrent run.
- Image builds pass `rm`/`forcerm`, so no intermediate container survives a build
  (bollard's defaults would keep one per Dockerfile step), and the untagged images
  the classic builder leaves for earlier stages of a multi-stage build (the
  `data` stage) are removed — only the ones that build created.
- Containers that write into host bind mounts (the loadsmith run, `plugin install`
  into the cache, `--plugin` builds) run as the host `uid:gid`, so nothing the lab
  leaves on the host is root-owned.
- Temp dirs (`/output` and `/lab-summary` per case, the `--plugin` overlay and
  build target) are deleted at the end of the case / run.
- The `--summary-file` support probe (`loadsmith run --help`, once per image per
  run) uses a throwaway labelled container, removed right after.

## Image naming

A lab service image is addressed as an origin item, `<origin>/<name>`, which the
runner resolves (via `origin.rs`) to a build context directory and a local
Docker tag:

```
images/lab-postgres-15  →  context loadsmith-lab-canonical-images/images/lab-postgres-15/
                    →  tag     loadsmith-lab/images/lab-postgres-15:local
```

The item name *is* the directory name — no prefix stripping. The slash-separated
tag lines up with a registry-qualified tag (`<registry>/<origin>/<name>`). The
canonical images are already published by the `loadsmith-lab-canonical-images` CI
as `ghcr.io/loadsmith-el/<name>`, but the lab does not consume them yet: it always
builds locally.

## Build context and seed data

When the runner builds an image, it assembles an **in-memory tar** of the image
directory's files (`Dockerfile`, `init.sql`, …) — nothing else. This is done by
`build_dir_context_tar(context_dir, exclude)` in `loadsmith-lab-docker`; the tar
is built with the `tar` crate and streamed to the Docker daemon via `bollard`.

The canonical CSV is **not** in the build context — the image generates it
itself. The postgres `Dockerfile` is multi-stage:

```dockerfile
FROM python:3-slim AS data
RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen \
 && python /gen/generate.py            # → /gen/spacecraft_telemetry_events.csv
FROM postgres:15
COPY --from=data /gen/spacecraft_telemetry_events.csv /docker-entrypoint-initdb.d/events.csv
COPY init.sql ...
```

The `data` stage clones [`loadsmith-lab-canonical-data`](../canonical-data/overview.md)
at a pinned tag (`DATA_REF`, default `v1`; `DATA_REPO` overridable via
`--build-arg` for local/offline builds) and runs the deterministic stdlib-only
`generate.py`. The result is `COPY --from=data` into the postgres stage. Because
the dataset is a pure function of the generator, nothing is committed — the CSV is
reproduced byte-for-byte at build time.

**Tradeoff:** a *fresh* image build needs network (the clone + base images);
already-cached images and all *runtime* are offline. Under `buildx --platform
amd64,arm64` the `data` stage runs per arch, but the generator is deterministic so
every arch yields the identical CSV.

## The loadsmith image

Loadsmith always runs in a container. Its image is resolved by
`resolve_loadsmith_image`:

- **`--loadsmith <project dir>`** — builds `loadsmith:local` from that project's
  multi-stage `Dockerfile`. The build context is assembled in Rust
  by `build_source_context_tar(root, exclude)`, which walks the tree and skips
  `target/`, `.git/`, `docs/`, `definitions/`. (`--loadsmith <binary>` instead
  wraps the binary in a minimal `bookworm-slim` image.) Note: bollard's classic builder does
  **not** honor `.dockerignore` (the tar is already assembled), so the exclusion
  must happen in the Rust walker. The build is hermetic (compiles a static musl
  binary inside `rust:1-alpine`, cargo-chef cached, and ships it on the
  Dockerfile's default `slim` target), so it is correct regardless of the host's
  glibc.

- **default** — pulls the canonical published image
  `ghcr.io/loadsmith-el/loadsmith` (precedence: `--tag <t>` > a case's
  `loadsmith.image` > the rolling `:slim`) via cache → pull, erroring clearly if
  absent.

The loadsmith container is started on the case network with the output dir mounted
at `/output` and driven with
`run /case/pipeline.yaml --plugin-dir /plugins --log-level <level>`, plus
`--summary-file /lab-summary/summary.json` (a per-case scratch dir) when the
image supports it. Its stdout+stderr is streamed to the report gutter live via
`DockerClient::stream_logs(follow:true)`; the verdict is read from the JSON run
summary, or from that text for an engine without the flag. Each stream is also
kept on its own for `expect.logs`.

## DockerClient API

`loadsmith-lab-docker` wraps `bollard` with a typed async API:

```rust
pub struct DockerClient { /* bollard::Docker */ }

impl DockerClient {
    pub async fn new() -> Result<Self>;

    // Images
    pub async fn image_exists(&self, name: &str) -> Result<bool>;
    pub async fn pull_image(&self, name: &str) -> Result<()>;
    pub async fn build_image(&self, context_tar: Vec<u8>, tag: &str) -> Result<()>;

    // Networks
    pub async fn create_network(&self, name: &str) -> Result<String>;
    pub async fn remove_network(&self, name: &str) -> Result<()>;

    // Containers
    pub async fn run_container(&self, config: ContainerConfig) -> Result<String>;
    pub async fn wait_container(&self, id: &str) -> Result<i64>;
    pub async fn stop_and_remove(&self, id: &str) -> Result<()>;   // force + volumes
    pub async fn prune_stale(&self, older_than: &str) -> Result<()>; // label-scoped
    pub async fn container_logs(&self, id: &str) -> Result<String>;
    pub async fn stream_logs<F: FnMut(&str)>(&self, id: &str, on_line: F) -> Result<CapturedLogs>;
    pub async fn container_ip(&self, id: &str, net_name: &str) -> Result<String>;

    // Readiness
    pub async fn wait_tcp(&self, host: &str, port: u16, timeout: Duration) -> Result<()>;
}
```

`stream_logs` follows the container's stdout+stderr and invokes `on_line` for each
complete line as it arrives, returning a `CapturedLogs { combined, stdout, stderr }`
at exit (`combined` interleaves both streams in arrival order).

`ContainerConfig`:

```rust
pub struct ContainerConfig {
    pub image: String,
    pub hostname: Option<String>,
    pub network: Option<String>,
    pub env: Vec<(String, String)>,
    pub binds: Vec<String>,             // "host/path:/container/path"
    pub publish_ports: Vec<(u16, u16)>, // (host_port, container_port)
    pub cmd: Vec<String>,               // overrides the image CMD (appended to ENTRYPOINT)
    pub docker_args: Vec<String>,
    pub user: Option<String>,           // "uid:gid" — set when writing host bind mounts
}
```

There is also a free function `build_source_context_tar(root, exclude)` for
assembling a source-tree build context (used to build `loadsmith:local`).
