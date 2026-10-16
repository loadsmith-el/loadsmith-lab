use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use loadsmith_lab_docker::{ContainerConfig, DockerClient};
use loadsmith_lab_report::{print_gutter_line, print_prep_line, CaseResult};
use uuid::Uuid;

use crate::case::{load_case, Case, Expect, MysqlReadiness, OracleReadiness, PostgresReadiness};
use crate::image::{prepare_plugin_dir, resolve_image, resolve_loadsmith_image};
use crate::origin::{image_tag, resolve_item, Config, Kind};

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

    let (exit_code, loadsmith_stdout) = match inner {
        Ok(v) => v,
        Err(e) => {
            // On success the caller drops it after validation (bundle hooks
            // still need it); on failure nobody else knows it exists.
            let _ = std::fs::remove_dir_all(&output_dir);
            return Err(e);
        }
    };

    let duration = start.elapsed();
    let (rows_read, rows_written) = parse_rows_from_stdout(&loadsmith_stdout);
    let observed = Observed {
        exit_code,
        summary_status: parse_summary_status(&loadsmith_stdout),
        rows_read,
        rows_written,
    };
    let failures = validate_expects(&case.expect, &observed, &output_dir);

    Ok(CaseResult {
        name: case.case.name.clone(),
        description: case.case.description.clone(),
        passed: failures.is_empty(),
        duration,
        rows_read,
        rows_written,
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
) -> Result<(i64, String)> {
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

/// Runs loadsmith in a container on the case network and returns
/// `(exit_code, combined_logs)`. Logs are streamed to the gutter in real time.
async fn run_container(
    docker: &DockerClient,
    case: &Case,
    opts: &RunOpts,
    case_dir: &Path,
    net_name: &str,
    output_dir: &Path,
) -> Result<(i64, String)> {
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
    let cmd = vec![
        "run".to_string(),
        "/case/pipeline.yaml".to_string(),
        "--plugin-dir".to_string(),
        "/plugins".to_string(),
        "--log-level".to_string(),
        opts.log_level.clone(),
    ];

    let id = docker.run_container(ContainerConfig {
        image,
        hostname: None,
        network: Some(net_name.to_string()),
        env,
        binds,
        publish_ports: vec![],
        cmd,
        docker_args: case.loadsmith.docker_args.clone(),
        // Writes /output (a host bind mount): run as the host user so the
        // outputs aren't root-owned and can be cleaned up.
        user: crate::image::host_user(),
    }).await?;

    // Stream the container's combined output through the gutter live, and keep
    // the full text for row-count parsing. The stream ends when the container
    // exits, after which wait_container returns the exit code immediately.
    let logs = docker.stream_logs(&id, print_gutter_line).await.unwrap_or_default();
    let exit_code = docker.wait_container(&id).await?;
    let _ = docker.stop_and_remove(&id).await;

    // Save logs for row-count parsing (parse_rows_from_stdout reads from here).
    std::fs::write(output_dir.join("loadsmith.stdout"), logs.as_bytes())?;

    Ok((exit_code, logs))
}

/// What a loadsmith run reported, as checked against a case's `expect:`.
struct Observed {
    exit_code: i64,
    /// The summary box's `Status:` value (`success`/`error`), when printed.
    summary_status: Option<String>,
    rows_read: Option<u64>,
    rows_written: Option<u64>,
}

fn validate_expects(expect: &Expect, observed: &Observed, output_dir: &Path) -> Vec<String> {
    let mut failures = Vec::new();

    // Success needs BOTH a zero exit and (when printed) a `Status: success`
    // summary — older cores exited 0 even when the pipeline ended in error.
    let expected_success = expect.status == "success";
    let actual_success = observed.exit_code == 0
        && observed.summary_status.as_deref().is_none_or(|s| s == "success");
    if expected_success != actual_success {
        failures.push(format!(
            "status: expected '{}', got exit code {} (summary status: {})",
            expect.status,
            observed.exit_code,
            observed.summary_status.as_deref().unwrap_or("none")
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

/// The summary box's `Status:` line (e.g. `Status:       error`), lower-cased.
fn parse_summary_status(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| {
        let (_, value) = l.split_once("Status:")?;
        value.split_whitespace().next().map(str::to_ascii_lowercase)
    })
}

fn parse_rows_from_stdout(stdout: &str) -> (Option<u64>, Option<u64>) {
    (parse_metric(stdout, "rows read"), parse_metric(stdout, "rows written"))
}

fn parse_metric(s: &str, label: &str) -> Option<u64> {
    // Matches the loadsmith summary box lines like "Rows read:    100,000"
    // case-insensitively, then extracts the comma-formatted number.
    s.lines()
        .find(|l| l.to_ascii_lowercase().contains(label))
        .and_then(|l| l.split_whitespace().find(|w| {
            !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == ',')
        }))
        .and_then(|w| w.replace(',', "").parse().ok())
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

    const SUMMARY_ERROR: &str = "\
Pipeline:     p
Status:       error
Rows read:    0
Rows written: 0
";

    fn expect(status: &str, rows: Option<u64>) -> Expect {
        Expect { status: status.into(), rows_read: rows, rows_written: rows, output: None }
    }

    fn observed(stdout: &str, exit_code: i64) -> Observed {
        let (rows_read, rows_written) = parse_rows_from_stdout(stdout);
        Observed { exit_code, summary_status: parse_summary_status(stdout), rows_read, rows_written }
    }

    #[test]
    fn summary_status_is_parsed() {
        assert_eq!(parse_summary_status(SUMMARY_ERROR).as_deref(), Some("error"));
        assert_eq!(parse_summary_status("no summary here"), None);
    }

    #[test]
    fn error_summary_fails_even_with_exit_zero() {
        // Older cores printed `Status: error` but exited 0.
        let f = validate_expects(&expect("success", None), &observed(SUMMARY_ERROR, 0), Path::new("/x"));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("summary status: error"), "{f:?}");
    }

    #[test]
    fn row_counts_are_enforced() {
        let stdout = "Status:       success\nRows read:    3\nRows written: 4\n";
        let f = validate_expects(&expect("success", Some(4)), &observed(stdout, 0), Path::new("/x"));
        assert_eq!(f, vec!["rows_read: expected 4, got 3".to_string()]);
    }

    #[test]
    fn matching_run_passes() {
        let stdout = "Status:       success\nRows read:    100,000\nRows written: 100,000\n";
        let f = validate_expects(&expect("success", Some(100_000)), &observed(stdout, 0), Path::new("/x"));
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn expected_error_passes_on_failure() {
        let f = validate_expects(&expect("error", None), &observed(SUMMARY_ERROR, 1), Path::new("/x"));
        assert!(f.is_empty(), "{f:?}");
    }
}
