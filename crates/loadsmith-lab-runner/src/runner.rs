use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use loadsmith_lab_docker::{ContainerConfig, DockerClient};
use loadsmith_lab_report::{print_gutter_line, print_prep_line, CaseResult};
use uuid::Uuid;

use crate::case::{load_case, Case, Expect, PostgresReadiness};
use crate::image::{prepare_plugin_dir, resolve_image, resolve_loadsmith_image};
use crate::origin::{image_tag, resolve_item, Config, Kind};

pub struct RunOpts {
    /// The loadsmith core to run, when given: a path to a binary (wrapped in a
    /// minimal image) or a Rust project dir (built from its Dockerfile). When
    /// `None`, a published image is used (`loadsmith_tag` / the case's image).
    /// Loadsmith always runs in a container.
    pub loadsmith_source: Option<PathBuf>,
    /// Use the published image `loadsmith:<tag>` (only when `loadsmith_source`
    /// is `None`).
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

    // Run the fallible core, then always clean up regardless of outcome.
    let inner = run_case_inner(&docker, &case, opts, case_dir, &net_name, &mut svc_ids).await;

    // Primary cleanup: by container ID (tracked as containers start).
    for id in &svc_ids {
        let _ = docker.stop_and_remove(id).await;
    }
    // Fallback cleanup: by alias/name, guards against any ID tracking gap.
    for svc in &case.services {
        let _ = docker.stop_and_remove(&svc.alias).await;
    }
    let _ = docker.remove_network(&net_name).await;

    let (exit_code, loadsmith_stdout, output_dir) = inner?;

    let duration = start.elapsed();
    let failures = validate_expects(&case.expect, exit_code, &output_dir);
    let (rows_read, rows_written) = parse_rows_from_stdout(&loadsmith_stdout);

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
    svc_ids: &mut Vec<String>,
) -> Result<(i64, String, PathBuf)> {
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
            print_prep_line(&format!(
                "{} ready ({:.1}s)",
                svc.alias,
                ready_start.elapsed().as_secs_f64()
            ));
        }
    }

    let output_dir = std::env::temp_dir().join(format!("ls-lab-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&output_dir)?;

    let (exit_code, loadsmith_stdout) =
        run_container(docker, case, opts, case_dir, net_name, &output_dir).await?;

    Ok((exit_code, loadsmith_stdout, output_dir))
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
    // Build from local source (--local) or resolve a published image.
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
        binds.push(format!("{}:{}", vol.host, vol.container));
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

fn validate_expects(expect: &Expect, exit_code: i64, output_dir: &Path) -> Vec<String> {
    let mut failures = Vec::new();

    let expected_success = expect.status == "success";
    let actual_success = exit_code == 0;
    if expected_success != actual_success {
        failures.push(format!(
            "status: expected '{}', got exit code {}",
            expect.status, exit_code
        ));
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
