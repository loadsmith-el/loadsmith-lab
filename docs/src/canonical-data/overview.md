# Canonical Test Data

All lab cases share a single canonical dataset: **100,000 rows of synthetic
spacecraft telemetry**, generated deterministically with seed=42.

## Why a shared dataset

A shared dataset ensures that every case tests against the same data. If you add
a new destination plugin and write a lab case for it, you don't need to create a
new database schema or generate new data — the Postgres image already has
`spacecraft_telemetry_events` loaded. Your case just reads from it.

This also means that correctness is cross-validatable: if `catalog/postgres-to-jsonl`
produces 100,000 rows and `catalog/postgres-to-parquet-chunked` also produces 100,000
rows, both are reading the same source truth.

## The generator (no committed CSV)

The dataset is produced by a single generator in its own repo:

```
loadsmith-lab-canonical-data/generate.py
```

- **100,000 rows**, **34 columns** covering every Arrow-representable type
- **Seed=42** — fully deterministic; every run reproduces the identical ~43 MB CSV
- **stdlib-only** — no third-party dependencies

Because the CSV is a *pure function* of this deterministic generator, **it is
never committed** anywhere — storing it would just bloat git with a regenerable
artifact. Instead, each service image's Dockerfile clones
`loadsmith-lab-canonical-data` at a pinned tag and runs `generate.py` in a build
stage, then bakes the result in (see [The Docker Model](../architecture/docker-model.md)).
You do not need to generate anything by hand to run the existing cases — the image
build does it.

## What types it covers

The dataset is specifically designed to stress-test type mapping in EL tools:

| Category | Columns | Notes |
|---|---|---|
| Integer types | `reading_int` (INT32), `reading_bigint` (INT64), `status_code` (SMALLINT), `event_sequence` (BIGINT) | |
| Floating point | `reading_double` (DOUBLE PRECISION) | |
| Decimals | `reading_decimal` (18,6), `latitude` (9,6), `longitude` (9,6), `altitude_km` (12,3), `velocity_kmh` (12,3), `temperature_c` (8,3), `radiation_level` (10,5), `battery_percent` (5,2), `payload_mass_kg` (10,3) | Stored as Utf8 in Arrow |
| Boolean | `reading_bool`, `is_anomaly` | |
| Date | `event_date` (DATE) | Arrow Date32 |
| Time | `event_time` (TIME) | Stored as Utf8 in Arrow |
| Timestamp | `event_timestamp`, `received_at`, `created_at`, `updated_at`, `deleted_at` | Arrow Timestamp(ms) |
| Text | `id`, `spacecraft_id`, `mission_id`, `sensor_name`, `sensor_type`, `severity`, `reading_text`, `operator_notes`, `raw_payload_json`, `tags`, `checksum` | |
| Nulls | Most numeric/optional columns | ~30% null rate; `deleted_at` is 92% null |

Decimals are stored as Utf8 in Arrow because Arrow's Decimal128 type is not
natively represented in the Postgres binary wire protocol. The Postgres source
plugin uses the text protocol (`simple_query`) and stringifies all NUMERIC,
DECIMAL, and TIME values.

## Null handling

Null rates per column group:

| Columns | Null rate |
|---|---|
| `reading_int`, `reading_bigint`, `reading_decimal`, `reading_double`, `reading_bool`, `reading_text` | ~30% |
| `latitude`, `longitude`, `altitude_km`, `velocity_kmh`, `temperature_c`, `radiation_level`, `battery_percent`, `payload_mass_kg` | ~30% |
| `event_time` | ~15% |
| `received_at` | ~10% |
| `raw_payload_json` | ~40% |
| `checksum` | ~5% |
| `deleted_at` | ~92% |
| All others | 0% (NOT NULL) |

This distribution ensures that null paths are exercised in every Arrow builder
without making the dataset unrealistically sparse.

## Changing the data

The schema/generator is stable; change it only when the schema changes. The
generator lives in `loadsmith-lab-canonical-data` (stdlib-only). To change the
dataset:

1. Edit `generate.py` (and update the schema contract in its `README.md`, plus
   each image's `init.sql` to match).
2. Commit and **bump the tag** (e.g. `v2`).
3. Point images at the new ref: bump `DATA_REF` in each image's `Dockerfile`
   (or pass `--build-arg DATA_REF=v2` for a one-off build).

To preview the CSV locally without building an image:

```bash
cd loadsmith-lab-canonical-data && python generate.py   # → spacecraft_telemetry_events.csv (gitignored)
```

After changing the data, **rebuild the affected image** so the new CSV is baked
in (a cached image predating the change keeps the old data):

```bash
docker rmi loadsmith-lab/images/lab-postgres-15:local
./target/debug/loadsmith-lab build --select images/lab-postgres-15
```
