# Roadmap

What's shipped and what's queued next. Shipped items are documented in
[README.md](README.md) — no details repeated here.

## Shipped

- [x] Lab harness — runner, docker wrapper, CLI, report crates
- [x] `postgres-15` service image with baked-in canonical seed data
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

## Planned

- [ ] **More service images** — only `postgres-15` exists today; additional
      source services would extend lab coverage beyond the postgres source
      plugin.
