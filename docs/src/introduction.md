# Introduction

loadsmith-lab is the **integration test harness for [loadsmith](https://github.com/loadsmith-el/loadsmith)**. Its job is to answer one question with confidence: does loadsmith actually work correctly against a real database, with real data, end-to-end?

## Why a separate project?

Unit tests cover crates in isolation — they can verify that a type serializes correctly, that a cursor fetches the right number of rows, that the pump counts batches accurately. But they cannot answer:

- Does the Postgres source correctly handle all 34 column types in our schema, including NUMERIC, TIME, and DECIMAL?
- Does 100,000 rows make it from Postgres through the loadsmith pump to the JSONL output, without a single row dropped?
- Does the readiness probe wait long enough for the database to finish seeding before loadsmith connects?

Those questions require a real Postgres instance with real data and a real invocation of the `loadsmith` binary. loadsmith-lab provides exactly that.

## Design goals

- **Declarative cases.** Each test case is a directory with two YAML files: `case.yaml` (what services to start, what to assert) and `pipeline.yaml` (the loadsmith pipeline to run). No test code.

- **Real Docker services.** The lab spins up actual Docker containers for each required service. There are no mocks or stubs. If the Postgres image has a bug, the test fails.

- **Canonical, deterministic data.** All cases share a single seeded dataset — 100,000 rows of spacecraft telemetry with every Arrow-representable type. The data is baked into the service images at build time.

- **Always containerized; one execution path.** Loadsmith always runs inside a Docker container on the case network. `--loadsmith <binary|project>` runs a local core (a project is built hermetically in Docker; a binary is wrapped) and `--plugin <binary|project>` overrides a cached plugin; without them, a published image is used. No host-binary special-casing.

- **Self-contained teardown.** Every run creates an isolated Docker bridge network with a UUID name. Containers and networks are torn down after the run regardless of outcome.

## What a run looks like

```
loadsmith-lab v0.1.0   local loadsmith

▶ postgres-to-jsonl
  "Read 100k rows from PostgreSQL and write to JSONL"
  image loadsmith-lab/images/postgres-15:local already in cache
  waiting for postgres at 172.18.0.2:5432...
  postgres ready

  │ Loadsmith v0.1.0  ·  postgres → jsonl
  │
  │   batch   1    2,000 rows
  │   batch   2    4,000 rows
  │   batch   4    8,000 rows
  │   batch   8   16,000 rows
  │   batch  16   32,000 rows
  │   batch  32   64,000 rows
  │   batch  50  100,000 rows
  │
  │ ─────────────────────────────────────────────────────
  │ Pipeline:     postgres-to-jsonl-smoke
  │ Route:        loadsmith-source-postgres → loadsmith-destination-jsonl
  │ Status:       success
  │ Rows read:    100,000
  │ Rows written: 100,000
  │ Batches:      50
  │ Duration:     0:01:14
  │ Throughput:   1,351 rows/s
  │ ─────────────────────────────────────────────────────

  ✓ passed   100,000 rows read · 100,000 written   74.2s

────────────────────────────────────────────────────────
1 passed, 0 failed
```
