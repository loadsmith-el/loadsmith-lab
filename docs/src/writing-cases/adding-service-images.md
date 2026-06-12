# Adding Service Images

A service image is a Docker image that provides a dependency for a case — a
database, a message queue, a file server. The lab can use any public Docker image
directly, or build a custom image from a self-contained build context in the
**`loadsmith-lab-canonical-images`** repo (the "images" origin).

## Using an existing public image

If your service needs no seeding or customization, just reference the public image
directly in `case.yaml`:

```yaml
services:
  - image: redis:7
    alias: redis
    readiness:
      tcp: 6379
      timeout_seconds: 30
```

No Dockerfile is needed. The lab will pull `redis:7` from Docker Hub on first use.

## Building a custom lab image

Custom images live in `loadsmith-lab-canonical-images/images/<name>/` — just a `Dockerfile`
plus init files (no committed data). They're addressed as `images/<name>`
(origin/name) and built under a local tag:

```
images/lab-postgres-15   →   tag loadsmith-lab/images/lab-postgres-15:local
images/mysql-8        →   tag loadsmith-lab/images/mysql-8:local
```

The directory name *is* the item name — no prefix stripping. Add an entry under
`[images]` in `loadsmith-lab-canonical-images/loadsmith-lab.toml` so it shows up in the
manifest.

The image generates its own seed CSV at build time: a **multi-stage** Dockerfile
clones `loadsmith-lab-canonical-data` (pinned) and runs `generate.py`, then
`COPY --from=data` bakes the result in. Nothing is committed to the image repo.

### Creating a new image

**1. Create the directory:**

```bash
mkdir -p loadsmith-lab-canonical-images/images/mysql-8
touch loadsmith-lab-canonical-images/images/mysql-8/Dockerfile
touch loadsmith-lab-canonical-images/images/mysql-8/init.sql
```

**2. Write the multi-stage Dockerfile** (mirror `lab-postgres-15/Dockerfile`):

```dockerfile
# ── stage 1: generate the canonical CSV from the data repo (pinned) ──
FROM python:3-slim AS data
RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ARG DATA_REPO=https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git
ARG DATA_REF=v1
RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen \
 && python /gen/generate.py

# ── stage 2: the seeded service image ──
FROM mysql:8
COPY --from=data /gen/spacecraft_telemetry_events.csv /docker-entrypoint-initdb.d/events.csv
COPY init.sql   /docker-entrypoint-initdb.d/01_init.sql
ENV MYSQL_DATABASE=lab
ENV MYSQL_USER=lab
ENV MYSQL_PASSWORD=lab
ENV MYSQL_ROOT_PASSWORD=lab
```

The CSV arrives from the `data` stage as `events.csv` — your final stage always
`COPY --from=data … events.csv`. Pin `DATA_REF` to a tag for reproducibility.

**2b. Register it in the manifest** (`loadsmith-lab-canonical-images/loadsmith-lab.toml`):

```toml
[images]
mysql-8 = "MySQL 8 with the canonical seed data baked in"
```

**3. Write `init.sql`:**

```sql
USE lab;

CREATE TABLE spacecraft_telemetry_events (
    id           VARCHAR(36)  NOT NULL PRIMARY KEY,
    spacecraft_id VARCHAR(50) NOT NULL,
    event_sequence BIGINT     NOT NULL,
    -- ... all 34 columns
);

LOAD DATA INFILE '/var/lib/mysql-files/events.csv'
INTO TABLE spacecraft_telemetry_events
FIELDS TERMINATED BY ','
ENCLOSED BY '"'
LINES TERMINATED BY '\n'
IGNORE 1 ROWS;
```

**4. Reference the image in a case** (as an `<origin>/<name>` image reference):

```yaml
services:
  - image: images/mysql-8
    alias: mysql
    readiness:
      tcp: 3306
      timeout_seconds: 120
```

**5. Build it:**

```bash
./target/debug/loadsmith-lab build --select images/mysql-8
```

Or let it build automatically the first time a case that needs it runs.

## The Postgres image in detail

`loadsmith-lab-canonical-images/images/lab-postgres-15/` is the reference implementation for a
seeded lab image.

**`Dockerfile`** (multi-stage — a `data` stage generates the CSV, the postgres
stage bakes it in):
```dockerfile
FROM python:3-slim AS data
RUN apt-get update -qq && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ARG DATA_REPO=https://github.com/loadsmith-el/loadsmith-lab-canonical-data.git
ARG DATA_REF=v1
RUN git clone --depth 1 --branch "${DATA_REF}" "${DATA_REPO}" /gen && python /gen/generate.py

FROM postgres:15
COPY --from=data /gen/spacecraft_telemetry_events.csv /docker-entrypoint-initdb.d/events.csv
COPY init.sql   /docker-entrypoint-initdb.d/01_init.sql
ENV POSTGRES_DB=lab
ENV POSTGRES_USER=lab
ENV POSTGRES_PASSWORD=lab
```

- `COPY --from=data … events.csv` — the CSV is generated in the `data` stage and
  copied in, available inside the container at init time.
- `COPY init.sql` — named `01_init.sql` so Postgres runs it first among any
  init scripts (Postgres runs `*.sql` and `*.sh` files in the `initdb.d` directory
  alphabetically on first start).

**`init.sql`:**
```sql
CREATE TABLE spacecraft_telemetry_events (
    id              VARCHAR(36) PRIMARY KEY,
    spacecraft_id   VARCHAR(50) NOT NULL,
    mission_id      VARCHAR(50) NOT NULL,
    event_sequence  BIGINT      NOT NULL,
    -- ... 30 more columns
);

CREATE INDEX idx_spacecraft  ON spacecraft_telemetry_events (spacecraft_id);
CREATE INDEX idx_mission     ON spacecraft_telemetry_events (mission_id);
CREATE INDEX idx_sensor_type ON spacecraft_telemetry_events (sensor_type);
CREATE INDEX idx_timestamp   ON spacecraft_telemetry_events (event_timestamp);

COPY spacecraft_telemetry_events
FROM '/docker-entrypoint-initdb.d/events.csv'
WITH (FORMAT CSV, HEADER true, NULL '');
```

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
