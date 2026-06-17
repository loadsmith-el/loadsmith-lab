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
- [x] Smoke case `postgres-to-jsonl` (content + type round-trip validation)
- [x] Volume/throughput cases `postgres-to-null-{5M,15M}`
- [x] Smoke case `postgres-to-parquet-chunked` (compression + file-splitting validation)
- [x] Smoke case `postgres-to-parquet-single` (single-file Parquet output)
- [x] Local-core / published-image run modes (`--loadsmith <binary|project>`,
      `--plugin <binary|project>`, with a mounted plugin cache)
- [x] `/create-source-image` and `/create-destination-plugin` scaffolds
- [x] Test bundles — sequenced cases with setup/validate/cleanup hooks that
      run in a per-bundle image (no host deps); example
      `parquet-destination` validates single-file vs. chunked Parquet
- [x] **Published docs + content-repo docs** — the lab mdbook is hosted on
      GitHub Pages (<https://loadsmith-el.github.io/loadsmith-lab/>), built and
      deployed on every push to `main` by
      [`docs.yml`](.github/workflows/docs.yml). The content repos
      (`loadsmith-lab-canonical-{catalog,images,data}`) each ship their own
      published mdbook too.

## Planned

- [ ] **More service images** — `lab-postgres-15` and `lab-mysql-8` exist today;
      additional source services would extend lab coverage further.
- [ ] **End-to-end "build an EL" guide** (shared with loadsmith) — author a
      pipeline, build an image with its plugins installed, and validate it with a
      lab case; the natural getting-started for a real pipeline.
