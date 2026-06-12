use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use loadsmith_lab_docker::{build_dir_context_tar, ContainerConfig, DockerClient};
use loadsmith_lab_report::{
    print_bundle_header, print_case_header, print_entry_result, print_gutter_line, print_hook_line,
    BundleEntryResult, BundleResult, CaseResult,
};
use serde::Deserialize;

use crate::origin::{resolve_item, Kind};
use crate::runner::{run_case, RunOpts};

/// A bundle: an ordered sequence of existing cases, each wrapped with optional
/// `setup` → run case → `validate` → `cleanup` hook scripts. The case itself is
/// untouched — a bundle only chains and wraps cases that already work standalone.
#[derive(Debug, Deserialize)]
pub struct Bundle {
    pub bundle: BundleMeta,
    pub cases: Vec<BundleEntry>,
}

#[derive(Debug, Deserialize)]
pub struct BundleMeta {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BundleEntry {
    /// An existing case, referenced as `<origin>/<name>` and resolved through
    /// the registered origins (installed copy or live local origin).
    pub case: String,
    /// Optional hook script paths, as they exist *inside the bundle image*
    /// (e.g. `/scripts/check.py`). Each is run as a one-shot container.
    pub setup: Option<String>,
    pub validate: Option<String>,
    pub cleanup: Option<String>,
}

pub fn load_bundle(path: &Path) -> Result<Bundle> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read bundle file {}: {e}", path.display()))?;
    serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("invalid bundle.yaml at {}: {e}", path.display()))
}

/// Builds the bundle's hook image from `bundle_dir`'s Dockerfile + scripts and
/// returns its tag. Always (re)built; Docker's layer cache keeps repeats fast
/// while still picking up script edits. Mirrors `resolve_loadsmith_image`.
pub async fn resolve_bundle_image(
    docker: &DockerClient,
    bundle_dir: &Path,
    name: &str,
) -> Result<String> {
    let tag = format!("loadsmith-lab-bundle-{name}:local");
    let tar = build_dir_context_tar(bundle_dir, &[])?;
    docker.build_image(tar, &tag).await?;
    Ok(tag)
}

/// Runs a bundle and returns the aggregated result. Live output (headers, hook
/// streams, case runs, per-entry verdicts) is printed as it happens; the
/// returned `BundleResult` feeds the final summary. Every entry runs regardless
/// of earlier failures so the report shows the full picture.
pub async fn run_bundle(bundle_path: &Path, opts: &RunOpts) -> Result<BundleResult> {
    let bundle = load_bundle(bundle_path)?;
    let bundle_dir = bundle_path.parent().unwrap();

    print_bundle_header(&bundle.bundle.name, bundle.bundle.description.as_deref());

    let docker = DockerClient::new().await?;

    // Build the hook image up front; the cache makes repeat runs cheap.
    let img_start = Instant::now();
    let image = resolve_bundle_image(&docker, bundle_dir, &bundle.bundle.name).await?;
    print_hook_line(&format!(
        "bundle image {image} ready ({:.1}s)",
        img_start.elapsed().as_secs_f64()
    ));

    let mut entries = Vec::new();
    for entry in &bundle.cases {
        let r = run_entry(&docker, &bundle.bundle.name, &image, entry, opts).await;
        entries.push(r);
    }

    Ok(BundleResult {
        name: bundle.bundle.name,
        entries,
    })
}

/// Runs one bundle entry through setup → case → validate → cleanup. Infra errors
/// are captured as failures (not propagated) so the bundle keeps going.
async fn run_entry(
    docker: &DockerClient,
    bundle_name: &str,
    image: &str,
    entry: &BundleEntry,
    opts: &RunOpts,
) -> BundleEntryResult {
    let mut result = BundleEntryResult {
        case_name: entry.case.clone(),
        ..Default::default()
    };

    print_case_header(&entry.case, None);

    // 1. setup — runs before the case, so no output dir exists yet.
    if let Some(setup) = &entry.setup {
        print_hook_line(&format!("setup: {setup}"));
        result.setup_failure = hook_failure(
            run_hook(docker, image, bundle_name, &entry.case, setup, None).await,
        );
    }

    // 2. case — reuse run_case unchanged. Skipped if setup failed.
    let mut output_dir: Option<PathBuf> = None;
    if result.setup_failure.is_none() {
        match resolve_item(&opts.origins, &entry.case, Kind::Cases) {
            Ok(case_dir) => match run_case(&case_dir.join("case.yaml"), opts).await {
                Ok(cr) => {
                    output_dir = Some(cr.output_dir.clone());
                    result.case_result = Some(cr);
                }
                Err(e) => {
                    result.case_result = Some(CaseResult {
                        name: entry.case.clone(),
                        passed: false,
                        duration: Duration::ZERO,
                        failures: vec![e.to_string()],
                        ..Default::default()
                    });
                }
            },
            Err(e) => result.setup_failure = Some(e.to_string()),
        }
    }

    let case_passed = result.case_result.as_ref().is_some_and(|c| c.passed);

    // 3. validate — only if the case passed; mounts the produced output dir.
    if case_passed {
        if let Some(validate) = &entry.validate {
            print_hook_line(&format!("validate: {validate}"));
            result.validate_failure = hook_failure(
                run_hook(docker, image, bundle_name, &entry.case, validate, output_dir.as_deref())
                    .await,
            );
        }
    }

    // 4. cleanup — always runs (best effort); a failure is a warning, not a fail.
    if let Some(cleanup) = &entry.cleanup {
        print_hook_line(&format!("cleanup: {cleanup}"));
        result.cleanup_warning = hook_failure(
            run_hook(docker, image, bundle_name, &entry.case, cleanup, output_dir.as_deref()).await,
        );
    }

    // 5. drop the temp output dir so chained entries don't pile up ls-lab-* dirs.
    if let Some(dir) = &output_dir {
        let _ = std::fs::remove_dir_all(dir);
    }

    result.passed =
        result.setup_failure.is_none() && case_passed && result.validate_failure.is_none();

    print_entry_result(&result);
    result
}

/// Maps a hook's run outcome to an optional failure message: `None` on a clean
/// exit 0, `Some(reason)` on a non-zero exit or an infra error.
fn hook_failure(outcome: Result<i64>) -> Option<String> {
    match outcome {
        Ok(0) => None,
        Ok(code) => Some(format!("exit {code}")),
        Err(e) => Some(e.to_string()),
    }
}

/// Runs a single hook script as a one-shot container from the bundle image,
/// streaming its output live. Returns the container exit code. When
/// `output_dir` is given (validate/cleanup), it is bind-mounted at `/output`
/// and passed both as `$LOADSMITH_LAB_OUTPUT_DIR` and as the script's argv[1].
async fn run_hook(
    docker: &DockerClient,
    image: &str,
    bundle_name: &str,
    case_name: &str,
    script: &str,
    output_dir: Option<&Path>,
) -> Result<i64> {
    let mut env = vec![
        ("LOADSMITH_LAB_BUNDLE_NAME".to_string(), bundle_name.to_string()),
        ("LOADSMITH_LAB_CASE_NAME".to_string(), case_name.to_string()),
    ];
    let mut binds = Vec::new();
    let mut cmd = vec![script.to_string()];

    if let Some(dir) = output_dir {
        env.push(("LOADSMITH_LAB_OUTPUT_DIR".to_string(), "/output".to_string()));
        binds.push(format!("{}:/output", dir.display()));
        cmd.push("/output".to_string());
    }

    let id = docker
        .run_container(ContainerConfig {
            image: image.to_string(),
            hostname: None,
            network: None,
            env,
            binds,
            publish_ports: vec![],
            cmd,
            docker_args: vec![],
        })
        .await?;

    let _ = docker.stream_logs(&id, print_gutter_line).await;
    let exit = docker.wait_container(&id).await?;
    let _ = docker.stop_and_remove(&id).await;
    Ok(exit)
}
