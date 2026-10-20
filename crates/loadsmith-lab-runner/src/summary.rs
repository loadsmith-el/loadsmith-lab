//! What a loadsmith run reported, read from the engine's machine-readable run
//! summary (`loadsmith run --summary-file`, schema v1) or — for engines that
//! predate it — scraped from the human report on stdout.
//!
//! The JSON document is the engine's covered interface (see the engine's CLI
//! reference, "Machine-readable run summary"); the human box is not, so the
//! text path is only a fallback for older published images.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use loadsmith_lab_docker::{ContainerConfig, DockerClient};
use serde::Deserialize;

/// The run-summary schema version this lab understands.
pub const SUMMARY_VERSION: u32 = 1;

/// Where the summary file is written inside the loadsmith container.
pub const SUMMARY_MOUNT: &str = "/lab-summary";
pub const SUMMARY_FILE: &str = "summary.json";

/// The fields of the run-summary document the lab uses. Unknown fields are
/// ignored (the schema only grows within a version).
#[derive(Debug, Clone, Deserialize)]
pub struct RunSummary {
    pub version: u32,
    #[serde(default)]
    pub loadsmith_version: Option<String>,
    pub status: String,
    pub exit_code: i64,
    #[serde(default)]
    pub cause: Option<String>,
    #[serde(default)]
    pub route: Vec<RouteEntry>,
    pub rows_read: u64,
    pub rows_written: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteEntry {
    pub kind: String,
    #[serde(rename = "type")]
    pub plugin_type: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// Which channel the run's outcome was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarySource {
    /// The `--summary-file` JSON document.
    Json,
    /// The human report on stdout (engine without `--summary-file`, or the
    /// document was not produced).
    Text,
}

/// What a loadsmith run reported, as checked against a case's `expect:`.
#[derive(Debug, Clone)]
pub struct Observed {
    /// The container's exit code.
    pub exit_code: i64,
    /// `success` / `error` / `cancelled`, when known.
    pub status: Option<String>,
    /// The root cause of a failed run, when known.
    pub cause: Option<String>,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    /// The summary document's own `exit_code` (JSON only), cross-checked
    /// against the container's.
    pub summary_exit_code: Option<i64>,
    /// The stages and their plugin versions, from `route[]` (JSON only).
    pub route: Vec<RouteEntry>,
    /// A document with a `version` the lab does not understand.
    pub unsupported_version: Option<u32>,
    pub source: SummarySource,
}

impl Observed {
    /// From a parsed run-summary document. A `version` the lab does not
    /// understand falls back to scraping `stdout`, flagged so the case fails.
    pub fn from_summary(exit_code: i64, s: RunSummary, stdout: &str) -> Self {
        if s.version != SUMMARY_VERSION {
            return Observed {
                unsupported_version: Some(s.version),
                ..Observed::from_text(exit_code, stdout)
            };
        }
        Observed {
            exit_code,
            status: Some(s.status.to_ascii_lowercase()),
            cause: s.cause,
            rows_read: Some(s.rows_read),
            rows_written: Some(s.rows_written),
            summary_exit_code: Some(s.exit_code),
            route: s.route,
            unsupported_version: None,
            source: SummarySource::Json,
        }
    }

    /// Scraped from the human report (older engines).
    pub fn from_text(exit_code: i64, stdout: &str) -> Self {
        Observed {
            exit_code,
            status: parse_summary_status(stdout),
            cause: parse_cause(stdout),
            rows_read: parse_metric(stdout, "rows read"),
            rows_written: parse_metric(stdout, "rows written"),
            summary_exit_code: None,
            route: vec![],
            unsupported_version: None,
            source: SummarySource::Text,
        }
    }

    /// Reads the summary document at `path` if there is one, else falls back
    /// to scraping `stdout`. A document that exists but cannot be parsed is
    /// reported as a warning and the text path is used.
    pub fn read(exit_code: i64, path: Option<&Path>, stdout: &str) -> Self {
        if let Some(path) = path {
            match std::fs::read_to_string(path) {
                Ok(text) => match parse_summary(&text) {
                    Ok(s) => return Observed::from_summary(exit_code, s, stdout),
                    Err(e) => tracing::warn!("cannot parse run summary {}: {e}", path.display()),
                },
                // The engine writes the document for every run that got past
                // argument parsing; a missing file means it never got there.
                Err(e) => tracing::debug!("no run summary at {}: {e}", path.display()),
            }
        }
        Observed::from_text(exit_code, stdout)
    }

    /// The route as `type version` per stage, in pipeline order, for the
    /// report, e.g. `postgres 0.2.1 → jsonl 0.2.1`. `None` when unknown.
    pub fn route_line(&self) -> Option<String> {
        if self.route.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .route
            .iter()
            .map(|r| match &r.version {
                Some(v) => format!("{} {v}", r.plugin_type),
                None => r.plugin_type.clone(),
            })
            .collect();
        Some(parts.join(" → "))
    }
}

pub fn parse_summary(text: &str) -> serde_json::Result<RunSummary> {
    serde_json::from_str(text)
}

/// The summary box's `Status:` line (e.g. `Status:       error`), lower-cased.
fn parse_summary_status(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| {
        let (_, value) = l.split_once("Status:")?;
        value.split_whitespace().next().map(str::to_ascii_lowercase)
    })
}

/// The summary box's `Cause:` line, else a setup failure's `error:` line.
fn parse_cause(stdout: &str) -> Option<String> {
    let after = |marker: &str| {
        stdout.lines().find_map(|l| {
            let (_, value) = l.split_once(marker)?;
            let v = value.trim().trim_end_matches(['│', '|']).trim();
            (!v.is_empty()).then(|| v.to_string())
        })
    };
    after("Cause:").or_else(|| {
        stdout.lines().find_map(|l| {
            let v = l.trim().strip_prefix("error:")?.trim();
            (!v.is_empty()).then(|| v.to_string())
        })
    })
}

fn parse_metric(s: &str, label: &str) -> Option<u64> {
    // Matches the loadsmith summary box lines like "Rows read:    100,000"
    // case-insensitively, then extracts the comma-formatted number.
    s.lines()
        .find(|l| l.to_ascii_lowercase().contains(label))
        .and_then(|l| {
            l.split_whitespace()
                .find(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == ','))
        })
        .and_then(|w| w.replace(',', "").parse().ok())
}

/// Per-image memo of [`supports_summary_file`]: an image can't change during a
/// lab run, and every case of a run usually shares one.
static SUPPORT: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

/// Whether the loadsmith `image` knows `run --summary-file`, probed once per
/// image by reading `loadsmith run --help` in a throwaway container. A version
/// check wouldn't do: unreleased engines built with `--loadsmith` carry the
/// last released version number. Any probe failure means "no" (text fallback).
pub async fn supports_summary_file(docker: &DockerClient, image: &str) -> bool {
    if let Some(known) = SUPPORT.lock().unwrap().as_ref().and_then(|m| m.get(image).copied()) {
        return known;
    }
    let supported = match probe_help(docker, image).await {
        Ok(help) => help_mentions_summary_file(&help),
        Err(e) => {
            tracing::debug!("probing {image} for --summary-file failed: {e:#}");
            false
        }
    };
    tracing::debug!("{image} supports --summary-file: {supported}");
    SUPPORT
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(image.to_string(), supported);
    supported
}

async fn probe_help(docker: &DockerClient, image: &str) -> anyhow::Result<String> {
    let id = docker
        .run_container(ContainerConfig {
            image: image.to_string(),
            hostname: None,
            network: None,
            env: vec![("NO_COLOR".into(), "1".into())],
            binds: vec![],
            publish_ports: vec![],
            cmd: vec!["run".into(), "--help".into()],
            docker_args: vec![],
            user: None,
        })
        .await?;
    let result = async {
        docker.wait_container(&id).await?;
        docker.container_logs(&id).await
    }
    .await;
    let _ = docker.stop_and_remove(&id).await;
    result
}

fn help_mentions_summary_file(help: &str) -> bool {
    help.contains("--summary-file")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC_SUCCESS: &str = r#"{
  "version": 1,
  "loadsmith_version": "0.3.0",
  "pipeline": "orders",
  "status": "success",
  "exit_code": 0,
  "cause": null,
  "signal": null,
  "dry_run": false,
  "route": [
    {"kind": "source", "type": "postgres", "name": "loadsmith-source-postgres",
     "version": "0.2.1", "path": "/plugins/loadsmith-source-postgres"},
    {"kind": "destination", "type": "jsonl", "name": null, "version": null, "path": null}
  ],
  "rows_read": 100000,
  "rows_written": 100000,
  "batches": 50,
  "duration_ms": 3137,
  "throughput_rows_per_sec": 31877,
  "started_at": "2026-10-20T14:03:08.398Z",
  "finished_at": "2026-10-20T14:03:11.535Z",
  "state": null,
  "sink": null,
  "a_future_field": {"ignored": true}
}"#;

    const DOC_SETUP_ERROR: &str = r#"{"version":1,"loadsmith_version":"0.3.0","pipeline":null,"status":"error","exit_code":3,"cause":"config error: unknown key 'sorce'","signal":null,"dry_run":false,"route":[],"rows_read":0,"rows_written":0,"batches":0,"duration_ms":3,"throughput_rows_per_sec":0,"started_at":"2026-10-20T14:03:08.398Z","finished_at":"2026-10-20T14:03:08.401Z","state":null,"sink":null}"#;

    #[test]
    fn json_document_is_read() {
        let o = Observed::from_summary(0, parse_summary(DOC_SUCCESS).unwrap(), "");
        assert_eq!(o.source, SummarySource::Json);
        assert_eq!(o.status.as_deref(), Some("success"));
        assert_eq!((o.rows_read, o.rows_written), (Some(100_000), Some(100_000)));
        assert_eq!(o.summary_exit_code, Some(0));
        assert_eq!(
            o.route_line().as_deref(),
            Some("postgres 0.2.1 → jsonl")
        );
    }

    #[test]
    fn setup_failure_document_carries_cause() {
        let o = Observed::from_summary(3, parse_summary(DOC_SETUP_ERROR).unwrap(), "");
        assert_eq!(o.status.as_deref(), Some("error"));
        assert_eq!(o.cause.as_deref(), Some("config error: unknown key 'sorce'"));
        assert_eq!(o.route_line(), None);
    }

    #[test]
    fn unknown_version_is_flagged() {
        let doc = DOC_SUCCESS.replace("\"version\": 1", "\"version\": 2");
        let o = Observed::from_summary(0, parse_summary(&doc).unwrap(), "Rows read:    5\n");
        assert_eq!(o.unsupported_version, Some(2));
        assert_eq!(o.source, SummarySource::Text);
        assert_eq!(o.rows_read, Some(5));
    }

    #[test]
    fn read_prefers_file_and_falls_back_to_text() {
        let dir = std::env::temp_dir().join(format!("ls-lab-summary-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stdout = "Status:       error\nRows read:    7\nRows written: 0\nCause:        boom\n";

        // Present → JSON wins over the (contradicting) human text.
        let file = dir.join("summary.json");
        std::fs::write(&file, DOC_SUCCESS).unwrap();
        let o = Observed::read(0, Some(&file), stdout);
        assert_eq!(o.source, SummarySource::Json);
        assert_eq!(o.rows_read, Some(100_000));

        // Missing (older engine / usage error) → text.
        let o = Observed::read(1, Some(&dir.join("absent.json")), stdout);
        assert_eq!(o.source, SummarySource::Text);
        assert_eq!(o.rows_read, Some(7));
        assert_eq!(o.cause.as_deref(), Some("boom"));

        // Garbage → text, not a crash.
        std::fs::write(&file, "{not json").unwrap();
        assert_eq!(Observed::read(1, Some(&file), stdout).source, SummarySource::Text);

        // Not requested (engine without --summary-file) → text.
        assert_eq!(Observed::read(1, None, stdout).source, SummarySource::Text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_status_rows_and_cause_are_scraped() {
        let o = Observed::from_text(0, "Status:       success\nRows read:    100,000\nRows written: 99\n");
        assert_eq!(o.status.as_deref(), Some("success"));
        assert_eq!((o.rows_read, o.rows_written), (Some(100_000), Some(99)));
        assert_eq!(o.cause, None);
        assert_eq!(Observed::from_text(0, "no summary here").status, None);
    }

    #[test]
    fn text_cause_falls_back_to_error_line() {
        let o = Observed::from_text(3, "2026 INFO starting\nerror: invalid pipeline: unknown key\n");
        assert_eq!(o.cause.as_deref(), Some("invalid pipeline: unknown key"));
    }

    #[test]
    fn help_probe_detects_flag() {
        assert!(help_mentions_summary_file(
            "Options:\n      --summary-file <SUMMARY_FILE>  Also write the JSON run summary\n"
        ));
        // v0.2.0's `run --help` has no such flag.
        assert!(!help_mentions_summary_file("Options:\n      --plugin-dir <PLUGIN_DIR>\n"));
    }
}
