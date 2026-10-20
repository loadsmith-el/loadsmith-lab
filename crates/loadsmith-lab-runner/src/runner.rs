use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use loadsmith_lab_docker::{ContainerConfig, DockerClient};
use loadsmith_lab_report::{print_gutter_line, print_prep_line, CaseResult};
use uuid::Uuid;

use crate::case::{load_case, Case, Expect, MysqlReadiness, OracleReadiness, PostgresReadiness};
use crate::image::{prepare_plugin_dir, resolve_image, resolve_loadsmith_image};
use crate::logs::{check_logs, Captured, Masker};
use crate::origin::{image_tag, resolve_item, Config, Kind};
use crate::summary::{
    supports_summary_file, Observed, SUMMARY_FILE, SUMMARY_MOUNT, SUMMARY_VERSION,
};

pub struct RunOpts {
    /// The loadsmith core to run, when given: a path to a binary (wrapped in a
    /// minimal image) or a Rust project dir (built from its Dockerfile). When
    /// `None`, the canonical published image is pulled (see `loadsmith_tag`).
    /// Loadsmith always runs in a container.
    pub loadsmith_source: Option<PathBuf>,
    /// Pick the version tag of the canonical published image
    /// `ghcr.io/loadsmith-el/loadsmith:<tag>` (only when `loadsmith_source` is
    /// `None`; otherwise the rolling `:slim` is pulled).
    pub loadsmith_tag: Option<String>,
    /// Registered origins, used to resolve a case's service-image references
    /// (`<origin>/<name>`) to a build context.
    pub origins: Config,
    /// Emit ANSI colour; propagated to loadsmith via NO_COLOR when false.
    pub color: bool,
    /// Log level forwarded to loadsmith (so `--log-level debug` shows the
    /// handshake/protocol negotiation in the framed output).
    pub log_level: String,
    /// Paths to locally-built plugin binaries (the `--plugin` flag) that
    /// override the cached canonical ones — for testing a plugin you're
    /// developing against released versions of the rest.
    pub plugin_overrides: Vec<PathBuf>,
}

/// Removes lab containers/networks left behind by earlier runs that were
/// interrupted (Ctrl-C, crash) before their own cleanup. Only resources older
/// than an hour are touched, so a concurrent run is never affected. Best effort.
pub async fn prune_stale_resources() {
    if let Ok(docker) = DockerClient::new().await {
        if let Err(e) = docker.prune_stale("1h").await {
            tracing::debug!("pruning stale lab resources failed: {e}");
        }
    }
}

/// Runs the case at `case_path` and returns the result.
///
/// Prep lines and loadsmith output are streamed to stdout in real time so the
/// user sees progress immediately. `CaseResult` fields that are already printed
/// live (`prep_lines`, `loadsmith_output`) are left empty.
pub async fn run_case(case_path: &Path, opts: &RunOpts) -> Result<CaseResult> {
    let case = load_case(case_path)?;
    let case_dir = case_path.parent().unwrap();
    let start = Instant::now();

    let docker = DockerClient::new().await?;
    let net_name = format!("ls-lab-{}", Uuid::new_v4().simple());
    let mut svc_ids: Vec<String> = Vec::new();

    docker.create_network(&net_name).await?;

    // Created up front so a failure anywhere below can still remove it.
    let output_dir = std::env::temp_dir().join(format!("ls-lab-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&output_dir)?;

    // Run the fallible core, then always clean up regardless of outcome.
    let inner =
        run_case_inner(&docker, &case, opts, case_dir, &net_name, &output_dir, &mut svc_ids).await;

    // Primary cleanup: by container ID (tracked as containers start).
    for id in &svc_ids {
        let _ = docker.stop_and_remove(id).await;
    }
    // Fallback cleanup: by alias/name, guards against any ID tracking gap.
    for svc in &case.services {
        let _ = docker.stop_and_remove(&svc.alias).await;
    }
    let _ = docker.remove_network(&net_name).await;

    let (observed, captured) = match inner {
        Ok(v) => v,
        Err(e) => {
            // On success the caller drops it after validation (bundle hooks
            // still need it); on failure nobody else knows it exists.
            let _ = std::fs::remove_dir_all(&output_dir);
            return Err(e);
        }
    };

    let duration = start.elapsed();
    let mut failures = validate_expects(&case.expect, &observed, &output_dir);
    if let Some(logs) = &case.expect.logs {
        failures.extend(check_logs(logs, &captured));
    }
    // A leaked secret may also surface in e.g. the `cause:` line: never echo it.
    let masker = Masker::new(case.expect.logs.as_ref());
    let failures = failures.into_iter().map(|f| masker.mask(&f).into_owned()).collect::<Vec<_>>();

    Ok(CaseResult {
        name: case.case.name.clone(),
        description: case.case.description.clone(),
        passed: failures.is_empty(),
        duration,
        rows_read: observed.rows_read,
        rows_written: observed.rows_written,
        route: observed.route_line(),
        // Prep lines and output were already printed live; leave empty so
        // print_result only prints the verdict.
        prep_lines: vec![],
        loadsmith_output: String::new(),
        failures,
        // Surfaced so bundle hooks can mount the produced files; the plain
        // `run` path ignores it.
        output_dir,
    })
}

async fn run_case_inner(
    docker: &DockerClient,
    case: &Case,
    opts: &RunOpts,
    case_dir: &Path,
    net_name: &str,
    output_dir: &Path,
    svc_ids: &mut Vec<String>,
) -> Result<(Observed, Captured)> {
    for svc in &case.services {
        // The case references its service image as an origin item
        // (`<origin>/<name>`); resolve the build context and derive the local
        // build tag.
        let context_dir = resolve_item(&opts.origins, &svc.image, Kind::Images)?;
        let tag = image_tag(&svc.image);

        let img_start = Instant::now();
        let built = resolve_image(docker, &tag, &context_dir).await?;
        if built {
            // Print build time immediately so the user sees it in real time.
            print_prep_line(&format!(
                "built {} in {:.1}s",
                svc.image,
                img_start.elapsed().as_secs_f64()
            ));
        }

        let id = docker.run_container(ContainerConfig {
            image: tag.clone(),
            hostname: Some(svc.alias.clone()),
            network: Some(net_name.to_string()),
            env: svc.env.iter().map(|e| {
                let mut parts = e.splitn(2, '=');
                (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string())
            }).collect(),
            binds: svc.docker_args.clone(),
            publish_ports: vec![],
            cmd: vec![],
            docker_args: vec![],
            user: None,
        }).await?;

        svc_ids.push(id.clone());

        if let Some(r) = &svc.readiness {
            // loadsmith and the services share the Docker network, but the lab
            // probes readiness from the host — use the container's bridge IP,
            // since the alias is only resolvable inside the network.
            let probe_host = docker.container_ip(&id, net_name).await?;
            print_prep_line(&format!("waiting for {} at {probe_host}:{}…", svc.alias, r.tcp));
            let ready_start = Instant::now();
            docker.wait_tcp(&probe_host, r.tcp, Duration::from_secs(r.timeout_seconds)).await?;

            if let Some(pg) = &r.postgres {
                wait_postgres(&probe_host, r.tcp, pg, Duration::from_secs(r.timeout_seconds)).await?;
            }
            if let Some(my) = &r.mysql {
                wait_mysql(&probe_host, r.tcp, my, Duration::from_secs(r.timeout_seconds)).await?;
            }
            if let Some(or) = &r.oracle {
                wait_oracle(&probe_host, r.tcp, or, Duration::from_secs(r.timeout_seconds)).await?;
            }
            print_prep_line(&format!(
                "{} ready ({:.1}s)",
                svc.alias,
                ready_start.elapsed().as_secs_f64()
            ));
        }
    }

    run_container(docker, case, opts, case_dir, net_name, output_dir).await
}

/// Runs loadsmith in a container on the case network and returns what it
/// reported. Logs are streamed to the gutter in real time; the outcome comes
/// from the JSON run summary (`--summary-file`) when the engine supports it,
/// else from the human report in those logs. The captured output is returned
/// alongside, for `expect.logs`.
async fn run_container(
    docker: &DockerClient,
    case: &Case,
    opts: &RunOpts,
    case_dir: &Path,
    net_name: &str,
    output_dir: &Path,
) -> Result<(Observed, Captured)> {
    // Build/wrap a local core (--loadsmith) or pull the canonical published image.
    let image = resolve_loadsmith_image(docker, opts, case).await?;

    // Plugins aren't bundled in the slim image — mount a host plugin dir
    // (cached canonical set + any `--plugin` local overrides) at /plugins.
    let plugin_dir = prepare_plugin_dir(docker, &image, &opts.plugin_overrides).await?;

    let pipeline_src = case_dir.join(&case.pipeline.file);
    let pipeline_dest = output_dir.join("pipeline.yaml");
    std::fs::copy(&pipeline_src, &pipeline_dest)?;

    // The output dir is mounted at /output; the pipeline writes there. Extra
    // case volumes are appended (they must not collide with /output).
    let mut binds = vec![
        format!("{}:/case/pipeline.yaml:ro", pipeline_dest.display()),
        format!("{}:/output", output_dir.display()),
        format!("{}:/plugins:ro", plugin_dir.display()),
    ];
    for vol in &case.loadsmith.volumes {
        // A relative `host` is resolved against the case dir so a case can mount
        // its own committed fixtures (e.g. a wallet) portably; an absolute path
        // is used as-is (Path::join returns the absolute arg unchanged).
        let host = case_dir.join(&vol.host);
        // Docker treats a non-absolute bind source as a named volume, so a
        // relative `--cases-dir` must not leak through: make it absolute.
        let host = std::path::absolute(&host).unwrap_or(host);
        binds.push(format!("{}:{}", host.display(), vol.container));
    }

    let mut env: Vec<(String, String)> = case.loadsmith.env.iter().map(|e| {
        let mut parts = e.splitn(2, '=');
        (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string())
    }).collect();
    if !opts.color {
        env.push(("NO_COLOR".to_string(), "1".to_string()));
    }

    // ENTRYPOINT is `loadsmith`; we supply the run command + flags as CMD.
    let mut cmd = vec![
        "run".to_string(),
        "/case/pipeline.yaml".to_string(),
        "--plugin-dir".to_string(),
        "/plugins".to_string(),
        "--log-level".to_string(),
        opts.log_level.clone(),
    ];

    // The machine-readable run summary goes to its own scratch mount (not
    // /output, which belongs to the pipeline and the bundle hooks); stdout
    // keeps the human report for the gutter. Engines older than the flag
    // (e.g. the published v0.2.0) would reject it with a usage error, so it
    // is only passed when `run --help` lists it.
    let summary_dir = if supports_summary_file(docker, &image).await {
        let dir = std::env::temp_dir().join(format!("ls-lab-summary-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir)?;
        binds.push(format!("{}:{SUMMARY_MOUNT}", dir.display()));
        cmd.push("--summary-file".to_string());
        cmd.push(format!("{SUMMARY_MOUNT}/{SUMMARY_FILE}"));
        Some(dir)
    } else {
        print_prep_line("loadsmith image predates --summary-file; reading its human report");
        None
    };

    let result = async {
        let id = docker.run_container(ContainerConfig {
            image,
            hostname: None,
            network: Some(net_name.to_string()),
            env,
            binds,
            publish_ports: vec![],
            cmd,
            docker_args: case.loadsmith.docker_args.clone(),
            // Writes /output and the summary dir (host bind mounts): run as the
            // host user so the files aren't root-owned and can be cleaned up.
            user: crate::image::host_user(),
        }).await?;

        // Stream the container's output through the gutter live (with any
        // `expect.logs.not_contains` secret masked), and keep the full text
        // for the fallback parser and `expect.logs`. The stream ends when the
        // container exits, after which wait_container returns immediately.
        let masker = Masker::new(case.expect.logs.as_ref());
        let logs = docker
            .stream_logs(&id, |line| print_gutter_line(&masker.mask(line)))
            .await
            .unwrap_or_default();
        let exit_code = docker.wait_container(&id).await?;
        let _ = docker.stop_and_remove(&id).await;

        // Kept beside the outputs for anyone inspecting a run by hand.
        std::fs::write(output_dir.join("loadsmith.stdout"), logs.combined.as_bytes())?;

        let summary_path = summary_dir.as_ref().map(|d| d.join(SUMMARY_FILE));
        let observed = Observed::read(exit_code, summary_path.as_deref(), &logs.combined);
        let captured = Captured {
            summary: summary_path.and_then(|p| std::fs::read_to_string(p).ok()),
            stdout: logs.stdout,
            stderr: logs.stderr,
        };
        Ok((observed, captured))
    }
    .await;

    if let Some(dir) = &summary_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    result
}

fn validate_expects(expect: &Expect, observed: &Observed, output_dir: &Path) -> Vec<String> {
    let mut failures = Vec::new();

    if let Some(v) = observed.unsupported_version {
        failures.push(format!(
            "run summary: version {v} is not supported (this lab reads version {SUMMARY_VERSION})"
        ));
    }

    // Success needs BOTH a zero exit and (when known) a `success` status —
    // older cores exited 0 even when the pipeline ended in error.
    let expected_success = expect.status == "success";
    let actual_success = observed.exit_code == 0
        && observed.status.as_deref().is_none_or(|s| s == "success");
    let status_failed = expected_success != actual_success;
    if status_failed {
        failures.push(format!(
            "status: expected '{}', got exit code {} (summary status: {})",
            expect.status,
            observed.exit_code,
            observed.status.as_deref().unwrap_or("none")
        ));
    }

    let exit_code_failed = expect.exit_code.is_some_and(|want| want != observed.exit_code);
    if let Some(want) = expect.exit_code.filter(|_| exit_code_failed) {
        failures.push(format!("exit_code: expected {want}, got {}", observed.exit_code));
    }

    // Why the run failed, when it wasn't supposed to (or failed differently).
    if status_failed || exit_code_failed {
        if let Some(cause) = observed.cause.as_deref().filter(|_| !actual_success) {
            failures.push(format!("cause: {cause}"));
        }
    }

    if let Some(doc_code) = observed.summary_exit_code.filter(|c| *c != observed.exit_code) {
        failures.push(format!(
            "run summary: exit_code {doc_code} disagrees with the container's exit code {}",
            observed.exit_code
        ));
    }

    for (label, want, got) in [
        ("rows_read", expect.rows_read, observed.rows_read),
        ("rows_written", expect.rows_written, observed.rows_written),
    ] {
        if let Some(want) = want {
            if got != Some(want) {
                let got = got.map_or("none".to_string(), |g| g.to_string());
                failures.push(format!("{label}: expected {want}, got {got}"));
            }
        }
    }

    if let Some(out) = &expect.output {
        let file_path = if out.file.starts_with('/') {
            PathBuf::from(&out.file)
        } else {
            output_dir.join(&out.file)
        };

        if !file_path.exists() {
            failures.push(format!("output file not found: {}", file_path.display()));
        } else if let Some(expected_count) = out.row_count {
            match count_lines(&file_path) {
                Ok(actual) if actual == expected_count => {}
                Ok(actual) => failures.push(format!(
                    "row_count: expected {expected_count}, got {actual} in {}",
                    file_path.display()
                )),
                Err(e) => failures.push(format!("cannot count rows in {}: {e}", file_path.display())),
            }
        }
    }

    failures
}

fn count_lines(path: &Path) -> Result<u64> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    Ok(reader.lines().count() as u64)
}

/// Waits until postgres accepts queries, going beyond just TCP-open.
/// Probes by attempting a real connection and running SELECT 1.
async fn wait_postgres(host: &str, port: u16, pg: &PostgresReadiness, timeout: Duration) -> Result<()> {
    use tokio::time::{sleep, Instant};
    use tokio_postgres::NoTls;

    let deadline = Instant::now() + timeout;
    let conn_str = format!(
        "host={host} port={port} dbname={} user={} password={} connect_timeout=3",
        pg.dbname, pg.user, pg.password
    );

    loop {
        let probe = pg.probe_query.as_deref().unwrap_or("SELECT 1");
        match tokio_postgres::connect(&conn_str, NoTls).await {
            Ok((client, conn_task)) => {
                tokio::spawn(conn_task);
                match client.query(probe, &[]).await {
                    Ok(rows) if !rows.is_empty() || probe == "SELECT 1" => {
                        tracing::info!("postgres at {host}:{port} is ready");
                        return Ok(());
                    }
                    Ok(_) => tracing::debug!("postgres probe returned 0 rows, waiting for data..."),
                    Err(e) => tracing::debug!("postgres query probe failed: {e}"),
                }
            }
            Err(e) => tracing::debug!("postgres connect probe failed: {e}"),
        }

        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for postgres at {host}:{port} to be ready");
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Waits until mysql accepts queries, going beyond just TCP-open.
/// Probes by attempting a real connection and running the probe query.
async fn wait_mysql(host: &str, port: u16, my: &MysqlReadiness, timeout: Duration) -> Result<()> {
    use mysql_async::prelude::Queryable;
    use tokio::time::{sleep, Instant};

    let deadline = Instant::now() + timeout;
    let probe = my.probe_query.as_deref().unwrap_or("SELECT 1");

    loop {
        let opts = mysql_async::OptsBuilder::default()
            .ip_or_hostname(host.to_string())
            .tcp_port(port)
            .user(Some(my.user.clone()))
            .pass(Some(my.password.clone()))
            .db_name(Some(my.dbname.clone()));

        match mysql_async::Conn::new(opts).await {
            Ok(mut conn) => {
                let result = conn.query::<mysql_async::Row, _>(probe).await;
                let _ = conn.disconnect().await;
                match result {
                    Ok(rows) if !rows.is_empty() || probe == "SELECT 1" => {
                        tracing::info!("mysql at {host}:{port} is ready");
                        return Ok(());
                    }
                    Ok(_) => tracing::debug!("mysql probe returned 0 rows, waiting for data..."),
                    Err(e) => tracing::debug!("mysql query probe failed: {e}"),
                }
            }
            Err(e) => tracing::debug!("mysql connect probe failed: {e}"),
        }

        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for mysql at {host}:{port} to be ready");
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Waits until oracle accepts queries, going beyond just TCP-open.
/// Probes by attempting a real (plaintext) connection and running the probe
/// query. Oracle takes a while to open the PDB and load the seed, so this is
/// what makes the difference between "listener up" and "data ready".
async fn wait_oracle(host: &str, port: u16, or: &OracleReadiness, timeout: Duration) -> Result<()> {
    use oracle_rs::{Config, Connection};
    use tokio::time::{sleep, Instant};

    let deadline = Instant::now() + timeout;
    let probe = or.probe_query.as_deref().unwrap_or("SELECT 1 FROM dual");

    loop {
        let config = Config::new(host, port, &or.service_name, &or.user, &or.password)
            .connect_timeout(Duration::from_secs(3));
        match Connection::connect_with_config(config).await {
            Ok(conn) => {
                let result = conn.query(probe, &[]).await;
                let _ = conn.close().await;
                match result {
                    Ok(r) if !r.rows.is_empty() || probe == "SELECT 1 FROM dual" => {
                        tracing::info!("oracle at {host}:{port} is ready");
                        return Ok(());
                    }
                    Ok(_) => tracing::debug!("oracle probe returned 0 rows, waiting for data..."),
                    Err(e) => tracing::debug!("oracle query probe failed: {e}"),
                }
            }
            Err(e) => tracing::debug!("oracle connect probe failed: {e}"),
        }

        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for oracle at {host}:{port} to be ready");
        }
        sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::parse_summary;

    const SUMMARY_ERROR: &str = "\
Pipeline:     p
Status:       error
Rows read:    0
Rows written: 0
Cause:        source error: connection refused
";

    fn expect(status: &str, rows: Option<u64>) -> Expect {
        Expect { status: status.into(), exit_code: None, rows_read: rows, rows_written: rows, output: None, logs: None }
    }

    fn text(stdout: &str, exit_code: i64) -> Observed {
        Observed::from_text(exit_code, stdout)
    }

    fn json(exit_code: i64, status: &str, rows: u64, cause: Option<&str>) -> Observed {
        let doc = serde_json::json!({
            "version": 1, "loadsmith_version": "0.3.0", "pipeline": "p",
            "status": status, "exit_code": exit_code, "cause": cause,
            "signal": null, "dry_run": false,
            "route": [{"kind": "source", "type": "postgres", "name": null, "version": "0.2.1", "path": null}],
            "rows_read": rows, "rows_written": rows, "batches": 1, "duration_ms": 1,
            "throughput_rows_per_sec": 0, "started_at": "x", "finished_at": "y",
            "state": null, "sink": null,
        });
        Observed::from_summary(exit_code, parse_summary(&doc.to_string()).unwrap(), "")
    }

    // ── text fallback (engines without --summary-file) ─────────────────────

    #[test]
    fn error_summary_fails_even_with_exit_zero() {
        // Older cores printed `Status: error` but exited 0.
        let f = validate_expects(&expect("success", None), &text(SUMMARY_ERROR, 0), Path::new("/x"));
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].contains("summary status: error"), "{f:?}");
    }

    #[test]
    fn text_failure_shows_cause() {
        let f = validate_expects(&expect("success", None), &text(SUMMARY_ERROR, 1), Path::new("/x"));
        assert_eq!(f.last().map(String::as_str), Some("cause: source error: connection refused"), "{f:?}");
    }

    #[test]
    fn row_counts_are_enforced() {
        let stdout = "Status:       success\nRows read:    3\nRows written: 4\n";
        let f = validate_expects(&expect("success", Some(4)), &text(stdout, 0), Path::new("/x"));
        assert_eq!(f, vec!["rows_read: expected 4, got 3".to_string()]);
    }

    #[test]
    fn matching_run_passes() {
        let stdout = "Status:       success\nRows read:    100,000\nRows written: 100,000\n";
        let f = validate_expects(&expect("success", Some(100_000)), &text(stdout, 0), Path::new("/x"));
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn expected_error_passes_on_failure() {
        let f = validate_expects(&expect("error", None), &text(SUMMARY_ERROR, 1), Path::new("/x"));
        assert!(f.is_empty(), "{f:?}");
    }

    // ── JSON run summary ───────────────────────────────────────────────────

    #[test]
    fn json_matching_run_passes() {
        let f = validate_expects(&expect("success", Some(10)), &json(0, "success", 10, None), Path::new("/x"));
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn json_failure_shows_cause() {
        let o = json(4, "error", 0, Some("plugin not found: loadsmith-source-pg"));
        let f = validate_expects(&expect("success", None), &o, Path::new("/x"));
        assert_eq!(
            f,
            vec![
                "status: expected 'success', got exit code 4 (summary status: error)".to_string(),
                "cause: plugin not found: loadsmith-source-pg".to_string(),
            ]
        );
    }

    #[test]
    fn json_row_counts_are_enforced() {
        let f = validate_expects(&expect("success", Some(11)), &json(0, "success", 10, None), Path::new("/x"));
        assert_eq!(f, vec!["rows_read: expected 11, got 10".to_string(), "rows_written: expected 11, got 10".to_string()]);
    }

    #[test]
    fn json_error_status_with_exit_zero_fails() {
        let f = validate_expects(&expect("success", None), &json(0, "error", 0, None), Path::new("/x"));
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn exit_code_expectation() {
        let mut e = expect("error", None);
        e.exit_code = Some(3);
        let ok = json(3, "error", 0, Some("config error: unknown key"));
        assert!(validate_expects(&e, &ok, Path::new("/x")).is_empty());

        let wrong = json(1, "error", 0, Some("source error: boom"));
        assert_eq!(
            validate_expects(&e, &wrong, Path::new("/x")),
            vec!["exit_code: expected 3, got 1".to_string(), "cause: source error: boom".to_string()]
        );
        // Works on the text path too: the container's exit code is always known.
        assert_eq!(validate_expects(&e, &text(SUMMARY_ERROR, 1), Path::new("/x")).len(), 2);
    }

    #[test]
    fn summary_exit_code_must_match_container() {
        let mut o = json(0, "success", 1, None);
        o.exit_code = 1;
        let f = validate_expects(&expect("error", None), &o, Path::new("/x"));
        assert!(f.iter().any(|l| l.contains("disagrees")), "{f:?}");
    }

    #[test]
    fn unsupported_summary_version_fails() {
        let mut o = json(0, "success", 1, None);
        o.unsupported_version = Some(2);
        let f = validate_expects(&expect("success", None), &o, Path::new("/x"));
        assert!(f[0].contains("version 2 is not supported"), "{f:?}");
    }
}
