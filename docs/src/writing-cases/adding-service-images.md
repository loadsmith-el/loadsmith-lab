# Adding Service Images

A service image is a Docker image that provides a dependency for a case — a
database, a message queue, a file server. Every service image is built from a
self-contained build context in an image origin, such as the
**`loadsmith-lab-canonical-images`** repo (the "images" origin).

## Using a public image

A case's `services[].image` is always an `<origin>/<name>` reference to an image
directory in a registered origin, so a public image like `redis:7` (or
`docker.io/redis:7`) cannot be referenced directly. To use one, wrap it in a
one-line image directory under an origin's `images/`:

```dockerfile
# images/lab-redis-7/Dockerfile
FROM redis:7
```

and reference it as `<origin>/<name>`:

```yaml
services:
  - image: images/lab-redis-7
    alias: redis
    readiness:
      tcp: 6379
      timeout_seconds: 30
```

## Building a custom lab image

Custom images live in `loadsmith-lab-canonical-images/images/<name>/` — just a `Dockerfile`
plus init files (no committed data). They're addressed as `images/<name>`
(origin/name) and built under a local tag:

```
images/lab-postgres-15   →   tag loadsmith-lab/images/lab-postgres-15:local
images/lab-mysql-8       →   tag loadsmith-lab/images/lab-mysql-8:local
```

The shipped images are `lab-postgres-15`, `lab-mysql-8`, `lab-oracle` and
`lab-mock-graph`. The directory name *is* the item name — no prefix stripping;
by convention it starts with `lab-`. Add an entry under `[images]` in
`loadsmith-lab-canonical-images/loadsmith-lab.toml` so it shows up in the
manifest.

The image generates its own seed CSV at build time: a **multi-stage** Dockerfile
clones `loadsmith-lab-canonical-data` (pinned) and runs `generate.py`, then
`COPY --from=data` bakes the result in. Nothing is committed to the image repo.

> **Publishing:** the `loadsmith-lab-canonical-images` CI (`build-images.yml`)
> builds every changed image directory on a push to `main` and publishes it to
> `ghcr.io/loadsmith-el/<image>` as `:latest`, `:sha-<sha>` and `:data-<ref>`
> (derived from the Dockerfile's `ARG DATA_REF`); a manual run with `force_all`
> rebuilds them all. Merging a new image to `main` therefore publishes it. The
> lab itself does not pull these yet — it still builds every image locally.

### Creating a new image

The steps below use a placeholder `lab-<service>`; the shipped `lab-mysql-8`
image is shown as the worked example for each file.

**1. Create the directory:**

```bash
mkdir -p loadsmith-lab-canonical-images/images/lab-<service>
touch loadsmith-lab-canonical-images/images/lab-<service>/Dockerfile
touch loadsmith-lab-canonical-images/images/lab-<service>/init.sql
```

**2. Write the multi-stage Dockerfile** (mirror an existing one). This is
`lab-mysql-8/Dockerfile`, comments trimmed:

```dockerfile
ARG DATA_REPO=https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git
ARG DATA_REF=v1

# ── stage 1: generate the canonical CSV from the data repo (pinned) ──
FROM python:3-slim AS data
RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ARG DATA_REPO
ARG DATA_REF
RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen \
 && python /gen/generate.py

# ── stage 2: the seeded mysql image ──
FROM mysql:8
ARG DATA_REF
LABEL org.opencontainers.image.version="${DATA_REF}"
# secure_file_priv dir, so the server-side LOAD DATA INFILE may read it
COPY --from=data /gen/spacecraft_telemetry_events.csv /var/lib/mysql-files/events.csv
COPY init.sql /docker-entrypoint-initdb.d/01_init.sql
# re-enables mysql_native_password (off by default on 8.4) for the lab_native user
COPY native-auth.cnf /etc/mysql/conf.d/native-auth.cnf
ENV MYSQL_DATABASE=lab
ENV MYSQL_USER=lab
ENV MYSQL_PASSWORD=lab
ENV MYSQL_ROOT_PASSWORD=lab
```

`DATA_REPO`/`DATA_REF` are declared globally (the CI derives the `:data-<ref>`
tag from `ARG DATA_REF=`) and re-declared in each stage that uses them. The CSV
arrives from the `data` stage as `spacecraft_telemetry_events.csv`; copy it
wherever your service's loader can read it. Pin `DATA_REF` to a tag for
reproducibility.

**2b. Register it in the manifest** (`loadsmith-lab-canonical-images/loadsmith-lab.toml`):

```toml
[images]
lab-<service> = "<Service> with the canonical seed data baked in"
```

**3. Write `init.sql`.** The loader must match the CSV the generator writes:
empty fields are NULL, text may be quoted, and lines end in CRLF (Python's
`csv` writer). The `lab-mysql-8` load, abridged:

```sql
LOAD DATA INFILE '/var/lib/mysql-files/events.csv'
INTO TABLE spacecraft_telemetry_events
FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '"' ESCAPED BY ''
LINES TERMINATED BY '\r\n'
IGNORE 1 LINES
(@id, @spacecraft_id, ...)       -- every column into a user variable
SET id = @id, ...;               -- then NULLIF empties / convert booleans
```

**4. Reference the image in a case** (as an `<origin>/<name>` image reference).
Add a query-level readiness probe that waits for the seed load — a TCP check
alone races the init script:

```yaml
services:
  - image: images/lab-mysql-8
    alias: mysql
    readiness:
      tcp: 3306
      timeout_seconds: 300
      mysql:
        dbname: lab
        user: lab
        password: lab
        probe_query: "SELECT 1 FROM spacecraft_telemetry_events LIMIT 1"
```

Probes exist for `postgres:`, `mysql:` and `oracle:` — see
[case.yaml](case-yaml.md).

**5. Build it:**

```bash
./target/debug/loadsmith-lab build --select images/lab-<service>
```

Or let it build automatically the first time a case that needs it runs.

## The Postgres image in detail

`loadsmith-lab-canonical-images/images/lab-postgres-15/` is the reference implementation for a
seeded lab image.

**`Dockerfile`** (multi-stage — a `data` stage generates the CSV, the postgres
stage bakes it in; comments trimmed):
```dockerfile
ARG DATA_REPO=https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git
ARG DATA_REF=v1

FROM python:3-slim AS data
RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ARG DATA_REPO
ARG DATA_REF
RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen \
 && python /gen/generate.py

FROM postgres:15
ARG DATA_REF
LABEL org.opencontainers.image.version="${DATA_REF}"
RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends openssl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=data /gen/spacecraft_telemetry_events.csv /docker-entrypoint-initdb.d/events.csv
COPY init.sql   /docker-entrypoint-initdb.d/01_init.sql
COPY 02_ssl.sh  /docker-entrypoint-initdb.d/02_ssl.sh
COPY 03_type_coverage.sql /docker-entrypoint-initdb.d/03_type_coverage.sql
RUN chmod 755   /docker-entrypoint-initdb.d/02_ssl.sh
ENV POSTGRES_DB=lab
ENV POSTGRES_USER=lab
ENV POSTGRES_PASSWORD=lab
```

- `COPY --from=data … events.csv` — the CSV is generated in the `data` stage and
  copied in, available inside the container at init time.
- `COPY init.sql` — named `01_init.sql` so Postgres runs it first among any
  init scripts (Postgres runs `*.sql` and `*.sh` files in the `initdb.d` directory
  alphabetically on first start).
- `02_ssl.sh` — a no-op unless the container is started with
  `LOADSMITH_ENABLE_SSL=on` (set it in the service's `env`); then it generates a
  self-signed server certificate and turns SSL on, for the TLS cases. `openssl`
  is installed in the image so this needs no network at runtime.
- `03_type_coverage.sql` — creates the extra type-coverage table used by the
  `*-types` cases.
- `LABEL org.opencontainers.image.version` — stamps the baked-in `DATA_REF`, so
  `docker inspect` reports the dataset revision.

**`init.sql`:**
```sql
CREATE TABLE spacecraft_telemetry_events (
    id              VARCHAR(36) PRIMARY KEY,
    spacecraft_id   VARCHAR(50) NOT NULL,
    mission_id      VARCHAR(50) NOT NULL,
    event_sequence  BIGINT      NOT NULL,
    -- ... 30 more columns
);

COPY spacecraft_telemetry_events
FROM '/docker-entrypoint-initdb.d/events.csv'
WITH (FORMAT CSV, HEADER true, NULL '');

CREATE INDEX idx_spacecraft  ON spacecraft_telemetry_events (spacecraft_id);
CREATE INDEX idx_mission     ON spacecraft_telemetry_events (mission_id);
CREATE INDEX idx_sensor_type ON spacecraft_telemetry_events (sensor_type);
CREATE INDEX idx_timestamp   ON spacecraft_telemetry_events (event_timestamp);
```

The indexes are created after the `COPY`, so the bulk load doesn't maintain them
row by row.

The `COPY` is transactional. All 100,000 rows are inserted in a single transaction
that commits when Postgres finishes initializing. The TCP port is open during the
`COPY`, so a simple TCP readiness check is not enough — use the `probe_query`
readiness field.

## Image build context

When building any lab image, the runner tars just the image directory's files —
no data is injected (the image generates its own):

```
{context_dir}/Dockerfile     ← from images/<name>/
{context_dir}/init.sql       ← from images/<name>/
```

The canonical CSV is produced inside the build by the `data` stage (which clones
`loadsmith-lab-canonical-data` and runs `generate.py`) and pulled into the final
image with `COPY --from=data … events.csv`.

## Rebuilding an image

Images are cached locally by Docker. If you change the Dockerfile or `init.sql`,
you need to rebuild:

```bash
docker rmi loadsmith-lab/images/lab-postgres-15:local
./target/debug/loadsmith-lab build --select images/lab-postgres-15
```

Or force a rebuild by removing the local image before running a case.
