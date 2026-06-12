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
so loadsmith reaches services by their alias hostnames. Because each run gets its
own network, multiple cases can run simultaneously without interfering.

The network is removed after teardown, even if the run fails — cleanup is
registered via `scopeguard::defer` before any containers are started.

## Container lifecycle

For each service declared in `case.yaml`:

1. **Image resolution** — the case references its service image as an
   `<origin>/<name>` reference (e.g. `images/lab-postgres-15`); the runner resolves
   that origin's build context and derives the local tag
   `loadsmith-lab/<origin>/<name>:local`, then tries in order:
   - Local Docker cache (image already exists): instant
   - Pull from registry: `docker pull <tag>`
   - Build from the resolved `Dockerfile`: auto-builds the first time

2. **Start** — `docker run` with:
   - Hostname set to the service alias (e.g., `--hostname pg`)
   - Attached to the case network
   - Environment variables from `env` field

3. **Readiness wait** — two levels, probed from the host against the container's
   bridge IP on the case network (the alias is only resolvable inside Docker):
   - TCP: poll `TcpStream::connect` every 500 ms until the port accepts connections
   - Postgres query (optional): connect to the database and run a probe query
     until it returns at least one row

4. **Teardown** — `docker stop` then `docker rm --force`. This runs unconditionally
   after the case finishes.

## Image naming

A lab service image is addressed as an origin item, `<origin>/<name>`, which the
runner resolves (via `origin.rs`) to a build context directory and a local
Docker tag:

```
images/lab-postgres-15  →  context loadsmith-lab-canonical-images/images/lab-postgres-15/
                    →  tag     loadsmith-lab/images/lab-postgres-15:local
```

The item name *is* the directory name — no prefix stripping. The slash-separated
tag lines up with a future registry-qualified tag (`<registry>/<origin>/<name>`)
once images are published via CI.

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
  must happen in the Rust walker. The build is hermetic (compiles inside Docker on a
  Debian base, ships on a matching Debian runtime), so it is correct regardless of
  the host's glibc.

- **default** — resolves the published image (`case.loadsmith.image`, or
  `loadsmith:<tag>` with `--tag`) via cache → pull, erroring clearly if absent.

The loadsmith container is started on the case network with the output dir mounted
at `/output` and driven with
`run /case/pipeline.yaml --plugin-dir /plugins --log-level <level>`. Its
combined stdout+stderr is streamed to the report gutter live via
`DockerClient::stream_logs(follow:true)`.

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
    pub async fn stop_and_remove(&self, id: &str) -> Result<()>;
    pub async fn container_logs(&self, id: &str) -> Result<String>;
    pub async fn stream_logs<F: FnMut(&str)>(&self, id: &str, on_line: F) -> Result<String>;
    pub async fn container_ip(&self, id: &str, net_name: &str) -> Result<String>;

    // Readiness
    pub async fn wait_tcp(&self, host: &str, port: u16, timeout: Duration) -> Result<()>;
}
```

`stream_logs` follows the container's combined stdout+stderr and invokes `on_line`
for each complete line as it arrives, returning the full accumulated text at exit.

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
}
```

There is also a free function `build_source_context_tar(root, exclude)` for
assembling a source-tree build context (used to build `loadsmith:local`).
