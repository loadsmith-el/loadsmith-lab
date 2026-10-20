# Roadmap

What's shipped and what's queued next. Shipped items are documented in
[README.md](README.md) — no details repeated here.

## Shipped

- [x] Lab harness — runner, docker wrapper, CLI, report crates
- [x] `lab-postgres-15` service image with baked-in canonical seed data
- [x] `lab-mysql-8` service image (baked-in canonical seed data) + mysql cases
      (`mysql-to-jsonl`, `mysql-to-jsonl-native`, `mysql-to-jsonl-tls-require`,
      `mysql-to-mysql` atomic + `staged-merge`) and a `mysql:` readiness probe.
      Covers both auth plugins: `caching_sha2_password` (default / MySQL 9) and
      legacy `mysql_native_password` (MySQL 5.x) via the `lab_native` user
- [x] `lab-oracle` service image (Oracle Free with baked-in canonical seed data
      plus an stunnel TLS terminator) and an `oracle:` readiness probe, with 7
      oracle cases (`oracle-to-jsonl`, `oracle-to-jsonl-tls`,
      `oracle-to-jsonl-incremental`, `oracle-to-oracle`,
      `oracle-to-oracle-staged-merge`, `oracle-to-oracle-lob`,
      `oracle-to-oracle-nls`)
- [x] `lab-mock-graph` service image (a mock Microsoft Graph endpoint serving the
      canonical data) with the `sharepoint-file-to-jsonl` and
      `sharepoint-list-to-jsonl` cases
- [x] Smoke case `postgres-to-jsonl` (content + type round-trip validation)
- [x] Postgres cases `postgres-to-jsonl-tls-require`,
      `postgres-to-parquet-localcopy`, `postgres-to-parquet-localcopy-resume`,
      `postgres-to-parquet-types` and `postgres-to-postgres-types`
- [x] Volume/throughput cases `postgres-to-null-{5M,15M}`
- [x] Smoke case `postgres-to-parquet-chunked` (compression + file-splitting validation)
- [x] Smoke case `postgres-to-parquet-single` (single-file Parquet output)
- [x] Local-core / published-image run modes (`--loadsmith <binary|project>`,
      `--plugin <binary|project>`, with a mounted plugin cache)
- [x] `/create-source-image` scaffold (`/create-destination-plugin` lives in the
      `loadsmith` repo)
- [x] Relative `loadsmith.volumes[].host` paths, resolved against the case dir
      (so a case can ship its own fixtures)
- [x] **Leaves nothing behind** — build `rm`/`forcerm`, container volumes removed
      on teardown, label-scoped pruning of stale resources from interrupted
      runs, host-user containers (no root-owned leftovers), temp dirs deleted,
      `--plugin` builds and the `--loadsmith` core built once per run (a failed
      build is reported with cargo's own error, once)
- [x] **`--plugin` projects build as static musl** in `rust:1-alpine`, the same
      build the canonical release ships (several projects each get their own
      target dir; a stale `Cargo.lock` under the read-only mount gets a hint)
- [x] **Strict expectations** — `rows_read`/`rows_written` are actually asserted
      (they were parsed but never checked), and `status: success` also requires
      the summary's `Status: success`
- [x] Test bundles — sequenced cases with setup/validate/cleanup hooks that
      run in a per-bundle image (no host deps); example
      `parquet-destination` validates single-file vs. chunked Parquet; also
      `postgres-types` (value-by-value type checks) and `tls-spike` (chains the
      TLS cases, no hooks)
- [x] **Published docs + content-repo docs** — the lab mdbook is hosted on
      GitHub Pages (<https://loadsmith-el.github.io/loadsmith-lab/>), built and
      deployed on every push to `main` that touches `docs/` by
      [`docs.yml`](.github/workflows/docs.yml). The content repos
      (`loadsmith-lab-canonical-{catalog,images,data}`) each ship their own
      published mdbook too.

## Planned

- [ ] **More service images** — `lab-postgres-15`, `lab-mysql-8`, `lab-oracle`
      and `lab-mock-graph` exist today;
      additional source services would extend lab coverage further.
- [ ] **End-to-end "build an EL" guide** (shared with loadsmith) — author a
      pipeline, build an image with its plugins installed, and validate it with a
      lab case; the natural getting-started for a real pipeline.
