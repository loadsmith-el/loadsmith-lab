# pipeline.yaml in a Case

Each case has a `pipeline.yaml` that is passed directly to the `loadsmith run`
command. It follows the standard [loadsmith pipeline YAML schema](https://loadsmith-el.github.io/loadsmith/reference/pipeline-yaml.html).

There is one important difference when writing a pipeline for a lab case: **use
service aliases as hostnames**, not `localhost`.

## Service aliases

In `case.yaml`, each service is declared with an `alias`:

```yaml
services:
  - image: images/postgres-15
    alias: pg
```

In `pipeline.yaml`, use that alias as the hostname:

```yaml
source:
  type: postgres
  config:
    host: pg        ← the alias from case.yaml
    port: 5432
```

Loadsmith runs in a container on the same Docker network as the services, so `pg`
resolves to the Postgres container via Docker DNS — exactly as written. No
rewriting happens; you use the alias directly. (The lab is identical with or
without `--loadsmith` here — `--loadsmith` only changes where the loadsmith core
comes from, not how it reaches services.)

## Output paths

The lab always mounts a fresh output directory at `/output` inside the loadsmith
container. Write your output under `/output`:

```yaml
destination:
  type: jsonl
  config:
    path: /output/events.jsonl
```

The lab validates the file on the host side of that mount: set
`expect.output.file: events.jsonl` (a path relative to the output dir). You don't
declare the `/output` mount in `case.yaml` — it's automatic. Use `loadsmith.volumes`
only for *additional* mounts.

## Complete example: postgres-to-jsonl

```yaml
pipeline:
  name: postgres-to-jsonl-smoke

source:
  type: postgres
  config:
    host: pg
    port: 5432
    dbname: lab
    user: lab
    password: lab
    query: "SELECT * FROM spacecraft_telemetry_events ORDER BY event_sequence"
    batch_size: 2000

destination:
  type: jsonl
  config:
    path: /output/events.jsonl
```

The `query` reads all 100,000 rows ordered by `event_sequence`, which produces
a deterministic output that is easy to validate.
