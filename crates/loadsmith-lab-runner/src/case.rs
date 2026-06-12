use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Case {
    pub case: CaseMeta,
    pub services: Vec<ServiceDef>,
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

fn default_timeout() -> u64 {
    60
}

#[derive(Debug, Deserialize)]
pub struct LoadsmithDef {
    pub image: String,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub docker_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct VolumeMount {
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
    pub rows_read: Option<u64>,
    pub rows_written: Option<u64>,
    pub output: Option<OutputExpect>,
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
