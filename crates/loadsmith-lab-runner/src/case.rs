use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Case {
    pub case: CaseMeta,
    pub services: Vec<ServiceDef>,
    #[serde(default)]
    pub loadsmith: LoadsmithDef,
    pub pipeline: PipelineDef,
    pub expect: Expect,
}

#[derive(Debug, Deserialize)]
pub struct CaseMeta {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ServiceDef {
    pub image: String,
    pub alias: String,
    pub readiness: Option<ReadinessDef>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub docker_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReadinessDef {
    pub tcp: u16,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Optional postgres-level readiness: wait until a query succeeds.
    pub postgres: Option<PostgresReadiness>,
    /// Optional mysql-level readiness: wait until a query succeeds.
    pub mysql: Option<MysqlReadiness>,
    /// Optional oracle-level readiness: wait until a query succeeds.
    pub oracle: Option<OracleReadiness>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PostgresReadiness {
    pub dbname: String,
    pub user: String,
    pub password: String,
    /// SQL that must return at least one row before the service is considered ready.
    /// Defaults to "SELECT 1" if not set (sufficient when no init data is needed).
    pub probe_query: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MysqlReadiness {
    pub dbname: String,
    pub user: String,
    pub password: String,
    /// SQL that must return at least one row before the service is considered ready.
    /// Defaults to "SELECT 1" if not set (sufficient when no init data is needed).
    pub probe_query: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OracleReadiness {
    /// Oracle service name (e.g. `FREEPDB1`).
    pub service_name: String,
    pub user: String,
    pub password: String,
    /// SQL that must return at least one row before the service is considered ready.
    /// Defaults to "SELECT 1 FROM dual" if not set. Probe the seeded table to wait
    /// until the data load (not just the listener) has finished.
    pub probe_query: Option<String>,
}

fn default_timeout() -> u64 {
    60
}

#[derive(Debug, Default, Deserialize)]
pub struct LoadsmithDef {
    /// Explicit per-case image override (a full ref). Normally omitted — the lab
    /// resolves the canonical published image instead (`--tag` picks the version,
    /// else the default `:slim`). See [`crate::image::resolve_loadsmith_image`].
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub docker_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct VolumeMount {
    /// Host path to bind-mount. A relative path is resolved against the case
    /// directory (so a case can ship its own fixtures); an absolute path is used
    /// as-is. See the mount loop in [`crate::runner`].
    pub host: String,
    pub container: String,
}

#[derive(Debug, Deserialize)]
pub struct PipelineDef {
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct Expect {
    pub status: String,
    /// The exact process exit code (engine exit-code table: 0/1/2/3/4/5/130/143),
    /// to tell failure classes apart — e.g. `3` for a configuration error.
    #[serde(default)]
    pub exit_code: Option<i64>,
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    pub output: Option<OutputExpect>,
    /// Assertions on the loadsmith container's captured output (stdout,
    /// stderr and the JSON run summary) — e.g. that a secret never leaks.
    #[serde(default)]
    pub logs: Option<LogsExpect>,
}

/// `expect.logs`: substrings that must (`contains`) or must not
/// (`not_contains`) appear in the loadsmith container's output. ANSI escapes
/// are stripped before matching. Unknown keys are rejected: a typo in a leak
/// check must not silently turn it off.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogsExpect {
    /// Each must appear in at least one of stdout, stderr or the run summary.
    #[serde(default)]
    pub contains: Vec<String>,
    /// None may appear in any of stdout, stderr or the run summary. Treated as
    /// secrets: never printed in full by the lab.
    #[serde(default)]
    pub not_contains: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct OutputExpect {
    pub file: String,
    pub row_count: Option<u64>,
}

pub fn load_case(path: &std::path::Path) -> anyhow::Result<Case> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read case file {}: {e}", path.display()))?;
    serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("invalid case.yaml at {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(expect: &str) -> Result<Expect, serde_yaml::Error> {
        serde_yaml::from_str(expect)
    }

    #[test]
    fn logs_expectation_is_optional_and_parsed() {
        assert!(parse("status: success\n").unwrap().logs.is_none());
        let e = parse(
            "status: success\nlogs:\n  not_contains: [\"s3cr3t\", \"BEGIN PRIVATE KEY\"]\n  contains: [\"schema negotiated\"]\n",
        )
        .unwrap();
        let logs = e.logs.unwrap();
        assert_eq!(logs.not_contains, vec!["s3cr3t", "BEGIN PRIVATE KEY"]);
        assert_eq!(logs.contains, vec!["schema negotiated"]);
        // Either list alone is fine.
        assert!(parse("status: success\nlogs:\n  contains: [x]\n").unwrap().logs.unwrap().not_contains.is_empty());
    }

    #[test]
    fn logs_typo_is_rejected() {
        // A misspelt leak check must fail loudly, not silently pass.
        assert!(parse("status: success\nlogs:\n  not_contain: [\"s3cr3t\"]\n").is_err());
    }
}
