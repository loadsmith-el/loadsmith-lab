use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use loadsmith_lab_report::{
    print_banner, print_bundle_summary, print_case_header, print_result, print_summary,
};
use loadsmith_lab_runner::origin::{self, Config};
use loadsmith_lab_runner::{run_bundle, run_case, Kind, RunOpts};

#[derive(Parser)]
#[command(
    name = "loadsmith-lab",
    version,
    about = "Loadsmith validation lab — run integration test cases against real services"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, default_value = "info")]
    log_level: String,

    #[arg(long, global = true, help = "Disable ANSI colour (also honours NO_COLOR)")]
    no_color: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run test cases (referenced as <origin>/<name>)
    Run(RunArgs),
    /// Run or list test bundles (sequenced cases with setup/validate/cleanup hooks)
    Bundle(BundleArgs),
    /// Build lab service images (referenced as <origin>/<name>)
    Build(BuildArgs),
    /// List available cases
    List(ListArgs),
    /// Manage origins — where cases/bundles/images come from
    Origin(OriginArgs),
    /// Install an item (<origin>/<name>) or a whole origin from a remote origin
    Install(InstallArgs),
    /// Remove an installed item (<origin>/<name>) from the workdir
    Uninstall(UninstallArgs),
}

#[derive(Args)]
struct BundleArgs {
    #[command(subcommand)]
    command: BundleCommands,
}

#[derive(Subcommand)]
enum BundleCommands {
    /// Run bundles
    Run(BundleRunArgs),
    /// List available bundles
    List(BundleListArgs),
}

#[derive(Args)]
struct BundleRunArgs {
    /// Run a specific bundle by name (<origin>/<name>)
    #[arg(long)]
    select: Option<String>,

    /// Run all bundles
    #[arg(long)]
    all: bool,

    /// Loadsmith core to run: a binary, or a Rust project dir to build (in a
    /// rust:bookworm container). Without it, uses --tag / the case's published image.
    #[arg(long, value_name = "PATH")]
    loadsmith: Option<PathBuf>,

    /// Use published image loadsmith:<tag> (ignored with --local)
    #[arg(long)]
    tag: Option<String>,

    /// Override a cached canonical plugin with a locally-built binary
    /// (repeatable; must be runnable in the loadsmith container)
    #[arg(long = "plugin", value_name = "PATH")]
    plugins: Vec<PathBuf>,

    /// Resolve bundles directly from this directory (ad-hoc, bypasses origins)
    #[arg(long)]
    bundles_dir: Option<PathBuf>,
}

#[derive(Args)]
struct BundleListArgs {
    /// List bundles directly from this directory (ad-hoc, bypasses origins)
    #[arg(long)]
    bundles_dir: Option<PathBuf>,
}

#[derive(Args)]
struct RunArgs {
    /// Run a specific case by name (<origin>/<name>)
    #[arg(long)]
    select: Option<String>,

    /// Run all cases
    #[arg(long)]
    all: bool,

    /// Loadsmith core to run: a binary, or a Rust project dir to build (in a
    /// rust:bookworm container). Without it, uses --tag / the case's published image.
    #[arg(long, value_name = "PATH")]
    loadsmith: Option<PathBuf>,

    /// Use published image loadsmith:<tag> (ignored with --local)
    #[arg(long)]
    tag: Option<String>,

    /// Override a cached canonical plugin with a locally-built binary, e.g.
    /// `--plugin ../loadsmith-canonical-plugins/target/release/loadsmith-destination-jsonl`
    /// (repeatable; the binary must be runnable in the loadsmith container)
    #[arg(long = "plugin", value_name = "PATH")]
    plugins: Vec<PathBuf>,

    /// Resolve cases directly from this directory (ad-hoc, bypasses origins)
    #[arg(long)]
    cases_dir: Option<PathBuf>,
}

#[derive(Args)]
struct BuildArgs {
    /// Build a specific image by name (<origin>/<name>, e.g. "images/postgres-15")
    #[arg(long)]
    select: Option<String>,

    /// Build all available service images
    #[arg(long)]
    all: bool,
}

#[derive(Args)]
struct ListArgs {
    /// List cases directly from this directory (ad-hoc, bypasses origins)
    #[arg(long)]
    cases_dir: Option<PathBuf>,

    /// Also show not-yet-installed cases offered by remote origins
    #[arg(long)]
    available: bool,
}

#[derive(Args)]
struct OriginArgs {
    #[command(subcommand)]
    command: OriginCommands,
}

#[derive(Subcommand)]
enum OriginCommands {
    /// List all registered origins (remote + local)
    List,
    /// Show an origin's manifest (the cases/bundles/images it offers)
    Show { name: String },
    /// Manage remote (git) origins
    Remote(OriginRemoteArgs),
    /// Manage local (path) origins, used live without installing
    Local(OriginLocalArgs),
}

#[derive(Args)]
struct OriginRemoteArgs {
    #[command(subcommand)]
    command: OriginRemoteCommands,
}

#[derive(Subcommand)]
enum OriginRemoteCommands {
    /// Register a remote origin and clone it
    Add { name: String, url: String },
    /// Refresh a remote origin's clone (git pull)
    Update {
        /// Origin to update (omit with --all)
        name: Option<String>,
        /// Update every remote origin
        #[arg(long)]
        all: bool,
    },
    /// Deregister a remote origin
    Rm {
        name: String,
        /// Also delete the cache clone from disk
        #[arg(long)]
        purge: bool,
    },
    /// List remote origins
    List,
}

#[derive(Args)]
struct OriginLocalArgs {
    #[command(subcommand)]
    command: OriginLocalCommands,
}

#[derive(Subcommand)]
enum OriginLocalCommands {
    /// Register a local origin (a path read live, never installed)
    Add { name: String, path: PathBuf },
    /// Deregister a local origin
    Rm { name: String },
    /// List local origins
    List,
}

#[derive(Args)]
struct InstallArgs {
    /// <origin>/<name> for one item, or just <origin> for everything it offers
    name: String,
}

#[derive(Args)]
struct UninstallArgs {
    /// <origin>/<name> of the installed item to remove
    name: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none();
    colored::control::set_override(color);

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&cli.log_level))
        .with_writer(std::io::stderr)
        .with_ansi(color)
        .init();

    match dispatch(cli, color).await {
        Ok(success) => {
            if success { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli, color: bool) -> Result<bool> {
    let log_level = cli.log_level.clone();
    let cfg = origin::load_config()?;

    match cli.command {
        Commands::Run(args) => cmd_run(args, &cfg, color, log_level).await,
        Commands::Bundle(args) => match args.command {
            BundleCommands::Run(a) => cmd_bundle_run(a, &cfg, color, log_level).await,
            BundleCommands::List(a) => cmd_bundle_list(a, &cfg),
        },
        Commands::Build(args) => cmd_build(args, &cfg).await,
        Commands::List(args) => cmd_list(args, &cfg),
        Commands::Origin(args) => cmd_origin(args, cfg),
        Commands::Install(args) => cmd_install(args, &cfg),
        Commands::Uninstall(args) => cmd_uninstall(args),
    }
}

/// Resolves the (qualified name, case.yaml path) pairs a run/list targets.
fn select_cases(
    cfg: &Config,
    select: &Option<String>,
    all: bool,
    cases_dir: &Option<PathBuf>,
) -> Result<Vec<(String, PathBuf)>> {
    if let Some(dir) = cases_dir {
        // Ad-hoc escape hatch: `dir` directly holds <name>/case.yaml subdirs.
        return select_from_dir(dir, "case.yaml", select, all);
    }
    if all {
        Ok(origin::discover(cfg, Kind::Cases)?
            .into_iter()
            .map(|(q, d)| (q, d.join("case.yaml")))
            .collect())
    } else {
        let sel = select
            .clone()
            .context("specify --select <origin>/<name> or --all")?;
        let dir = origin::resolve_item(cfg, &sel, Kind::Cases)?;
        Ok(vec![(sel, dir.join("case.yaml"))])
    }
}

/// Ad-hoc resolution against a bare collection directory (for --cases-dir/
/// --bundles-dir), using unqualified names.
fn select_from_dir(
    dir: &Path,
    manifest_file: &str,
    select: &Option<String>,
    all: bool,
) -> Result<Vec<(String, PathBuf)>> {
    if all {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(vec![]);
        };
        let mut out: Vec<(String, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                (name, e.path().join(manifest_file))
            })
            .filter(|(_, p)| p.exists())
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    } else {
        let name = select
            .clone()
            .context("specify --select <name> or --all")?;
        let path = dir.join(&name).join(manifest_file);
        anyhow::ensure!(path.exists(), "not found: {}", path.display());
        Ok(vec![(name, path)])
    }
}

async fn cmd_run(args: RunArgs, cfg: &Config, color: bool, log_level: String) -> Result<bool> {
    let cases = select_cases(cfg, &args.select, args.all, &args.cases_dir)?;
    anyhow::ensure!(!cases.is_empty(), "no matching cases found");

    let opts = RunOpts {
        loadsmith_source: args.loadsmith.clone(),
        loadsmith_tag: args.tag,
        origins: cfg.clone(),
        color,
        log_level,
        plugin_overrides: args.plugins.clone(),
    };

    let mode = if args.loadsmith.is_some() { "local loadsmith" } else { "remote image" };
    print_banner(env!("CARGO_PKG_VERSION"), mode);

    let mut results = vec![];
    for (name, case_path) in &cases {
        let description = peek_description(case_path);
        print_case_header(name, description.as_deref());
        match run_case(case_path, &opts).await {
            Ok(r) => {
                print_result(&r);
                results.push(r);
            }
            Err(e) => {
                let mut r = loadsmith_lab_report::CaseResult {
                    name: name.clone(),
                    passed: false,
                    failures: vec![e.to_string()],
                    ..Default::default()
                };
                r.duration = std::time::Duration::ZERO;
                print_result(&r);
                results.push(r);
            }
        }
    }

    print_summary(&results);
    Ok(results.iter().all(|r| r.passed))
}

async fn cmd_bundle_run(
    args: BundleRunArgs,
    cfg: &Config,
    color: bool,
    log_level: String,
) -> Result<bool> {
    let bundles = if let Some(dir) = &args.bundles_dir {
        select_from_dir(dir, "bundle.yaml", &args.select, args.all)?
    } else if args.all {
        origin::discover(cfg, Kind::Bundles)?
            .into_iter()
            .map(|(q, d)| (q, d.join("bundle.yaml")))
            .collect()
    } else {
        let sel = args
            .select
            .clone()
            .context("specify --select <origin>/<name> or --all")?;
        let dir = origin::resolve_item(cfg, &sel, Kind::Bundles)?;
        vec![(sel, dir.join("bundle.yaml"))]
    };
    anyhow::ensure!(!bundles.is_empty(), "no matching bundles found");

    let opts = RunOpts {
        loadsmith_source: args.loadsmith.clone(),
        loadsmith_tag: args.tag,
        origins: cfg.clone(),
        color,
        log_level,
        plugin_overrides: args.plugins.clone(),
    };

    let mode = if args.loadsmith.is_some() { "local loadsmith" } else { "remote image" };
    print_banner(env!("CARGO_PKG_VERSION"), mode);

    let mut all_passed = true;
    for (name, bundle_path) in &bundles {
        match run_bundle(bundle_path, &opts).await {
            Ok(result) => {
                if !result.entries.iter().all(|e| e.passed) {
                    all_passed = false;
                }
                print_bundle_summary(&result);
            }
            Err(e) => {
                eprintln!("error running bundle {name}: {e:#}");
                all_passed = false;
            }
        }
    }

    Ok(all_passed)
}

fn cmd_bundle_list(args: BundleListArgs, cfg: &Config) -> Result<bool> {
    let bundles = if let Some(dir) = &args.bundles_dir {
        select_from_dir(dir, "bundle.yaml", &None, true)?
    } else {
        origin::discover(cfg, Kind::Bundles)?
            .into_iter()
            .map(|(q, d)| (q, d.join("bundle.yaml")))
            .collect()
    };
    if bundles.is_empty() {
        println!("No bundles found.");
    } else {
        println!("Bundles:");
        for (name, _) in &bundles {
            println!("  {name}");
        }
    }
    Ok(true)
}

/// Cheaply reads just the case description for the header, before the run.
fn peek_description(case_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(case_path).ok()?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    value
        .get("case")?
        .get("description")?
        .as_str()
        .map(|s| s.to_string())
}

async fn cmd_build(args: BuildArgs, cfg: &Config) -> Result<bool> {
    use loadsmith_lab_docker::DockerClient;
    use loadsmith_lab_runner::resolve_image;

    let targets: Vec<(String, PathBuf)> = if args.all {
        origin::discover(cfg, Kind::Images)?
    } else if let Some(sel) = args.select {
        let dir = origin::resolve_item(cfg, &sel, Kind::Images)?;
        vec![(sel, dir)]
    } else {
        anyhow::bail!("specify --select <origin>/<name> or --all");
    };
    anyhow::ensure!(!targets.is_empty(), "no matching images found");

    let docker = DockerClient::new().await?;
    for (qualified, context_dir) in &targets {
        let tag = origin::image_tag(qualified);
        println!("building {qualified} -> {tag}...");
        resolve_image(&docker, &tag, context_dir).await?;
        println!("  done");
    }
    Ok(true)
}

fn cmd_list(args: ListArgs, cfg: &Config) -> Result<bool> {
    let cases = if let Some(dir) = &args.cases_dir {
        select_from_dir(dir, "case.yaml", &None, true)?
    } else {
        origin::discover(cfg, Kind::Cases)?
            .into_iter()
            .map(|(q, d)| (q, d.join("case.yaml")))
            .collect()
    };

    if cases.is_empty() {
        println!("No cases installed. Try `loadsmith-lab list --available`.");
    } else {
        println!("Cases:");
        for (name, _) in &cases {
            println!("  {name}");
        }
    }

    if args.available && args.cases_dir.is_none() {
        let available = origin::available(cfg, Kind::Cases)?;
        if !available.is_empty() {
            println!("\nAvailable (not installed):");
            for (name, desc) in &available {
                println!("  {name}  — {desc}");
            }
            println!("\nInstall with `loadsmith-lab install <origin>/<name>`.");
        }
    }
    Ok(true)
}

fn cmd_origin(args: OriginArgs, cfg: Config) -> Result<bool> {
    match args.command {
        OriginCommands::List => origin_list(&cfg),
        OriginCommands::Show { name } => origin_show(&cfg, &name),
        OriginCommands::Remote(a) => match a.command {
            OriginRemoteCommands::Add { name, url } => origin_remote_add(cfg, name, url),
            OriginRemoteCommands::Update { name, all } => origin_remote_update(&cfg, name, all),
            OriginRemoteCommands::Rm { name, purge } => origin_remote_rm(cfg, name, purge),
            OriginRemoteCommands::List => origin_remote_list(&cfg),
        },
        OriginCommands::Local(a) => match a.command {
            OriginLocalCommands::Add { name, path } => origin_local_add(cfg, name, path),
            OriginLocalCommands::Rm { name } => origin_local_rm(cfg, name),
            OriginLocalCommands::List => origin_local_list(&cfg),
        },
    }
}

fn origin_list(cfg: &Config) -> Result<bool> {
    if cfg.origins.is_empty() {
        println!("No origins registered.");
        return Ok(true);
    }
    println!("Origins:");
    for o in &cfg.origins {
        let kind = if o.is_git() { "remote" } else { "local " };
        println!("  {kind}  {:<16}  {}", o.name, o.location);
    }
    for hint in origin::update_hints(cfg) {
        println!("\n{hint}");
    }
    Ok(true)
}

fn origin_show(cfg: &Config, name: &str) -> Result<bool> {
    let manifest = origin::manifest_of(cfg, name)?;
    println!("Origin '{name}' offers:");
    for kind in [Kind::Cases, Kind::Bundles, Kind::Images] {
        let items = manifest.category(kind);
        if items.is_empty() {
            continue;
        }
        println!("\n{kind}:");
        for (item, desc) in items {
            println!("  {name}/{item}  — {desc}");
        }
    }
    Ok(true)
}

fn origin_remote_add(mut cfg: Config, name: String, url: String) -> Result<bool> {
    let replacing = cfg.find(&name).is_some();
    let o = origin::OriginConfig { name: name.clone(), source: "git".into(), location: url.clone() };
    // Clone immediately so the manifest is available right away.
    let cache = origin::origin_cache_dir(&name)?;
    if cache.exists() {
        std::fs::remove_dir_all(&cache)?;
    }
    println!("cloning '{name}' from {url}…");
    origin::clone_to_cache(&url, &cache)?;
    cfg.origins.retain(|o| o.name != name);
    cfg.origins.push(o);
    origin::save_config(&cfg)?;
    if replacing {
        println!("note: replaced existing origin '{name}'");
    }
    println!("added remote origin '{name}'");
    Ok(true)
}

fn origin_remote_update(cfg: &Config, name: Option<String>, all: bool) -> Result<bool> {
    let targets: Vec<&origin::OriginConfig> = if all {
        cfg.git_origins().collect()
    } else if let Some(name) = &name {
        let o = cfg.find(name).with_context(|| format!("no such origin: '{name}'"))?;
        vec![o]
    } else {
        anyhow::bail!("specify an origin name or --all");
    };
    for o in targets {
        println!("updating '{}'…", o.name);
        origin::update_origin(o)?;
    }
    Ok(true)
}

fn origin_remote_rm(mut cfg: Config, name: String, purge: bool) -> Result<bool> {
    let o = cfg.find(&name).with_context(|| format!("no such origin: '{name}'"))?;
    anyhow::ensure!(o.is_git(), "origin '{name}' is a local origin — use `origin local rm`");
    if purge {
        let cache = origin::origin_cache_dir(&name)?;
        if cache.exists() {
            std::fs::remove_dir_all(&cache)?;
        }
    }
    cfg.origins.retain(|o| o.name != name);
    origin::save_config(&cfg)?;
    println!("removed remote origin '{name}'");
    Ok(true)
}

fn origin_remote_list(cfg: &Config) -> Result<bool> {
    let mut any = false;
    for o in cfg.git_origins() {
        if !any {
            println!("Remote origins:");
            any = true;
        }
        let cache = origin::origin_cache_dir(&o.name)?;
        let state = if cache.exists() { "cloned" } else { "not cloned" };
        println!("  {:<16}  {}  [{state}]", o.name, o.location);
    }
    if !any {
        println!("No remote origins.");
    }
    Ok(true)
}

fn origin_local_add(mut cfg: Config, name: String, path: PathBuf) -> Result<bool> {
    let replacing = cfg.find(&name).is_some();
    let abs = path.canonicalize().with_context(|| format!("path does not exist: {}", path.display()))?;
    anyhow::ensure!(
        abs.join(origin::MANIFEST_FILE).exists(),
        "no {} at {} — not a valid origin",
        origin::MANIFEST_FILE,
        abs.display()
    );
    cfg.origins.retain(|o| o.name != name);
    cfg.origins.push(origin::OriginConfig {
        name: name.clone(),
        source: "path".into(),
        location: abs.to_string_lossy().to_string(),
    });
    origin::save_config(&cfg)?;
    if replacing {
        println!("note: replaced existing origin '{name}'");
    }
    println!("added local origin '{name}' -> {}", abs.display());
    Ok(true)
}

fn origin_local_rm(mut cfg: Config, name: String) -> Result<bool> {
    let o = cfg.find(&name).with_context(|| format!("no such origin: '{name}'"))?;
    anyhow::ensure!(o.is_path(), "origin '{name}' is a remote origin — use `origin remote rm`");
    cfg.origins.retain(|o| o.name != name);
    origin::save_config(&cfg)?;
    println!("removed local origin '{name}'");
    Ok(true)
}

fn origin_local_list(cfg: &Config) -> Result<bool> {
    let mut any = false;
    for o in cfg.path_origins() {
        if !any {
            println!("Local origins:");
            any = true;
        }
        let provides = origin::read_manifest(Path::new(&o.location))
            .map(|m| {
                format!(
                    "{} cases, {} bundles, {} images",
                    m.cases.len(),
                    m.bundles.len(),
                    m.images.len()
                )
            })
            .unwrap_or_else(|_| "unreadable manifest".to_string());
        println!("  {:<16}  {}  ({provides})", o.name, o.location);
    }
    if !any {
        println!("No local origins.");
    }
    Ok(true)
}

fn cmd_install(args: InstallArgs, cfg: &Config) -> Result<bool> {
    match args.name.split_once('/') {
        Some((origin_name, item)) => origin::install(cfg, origin_name, Some(item))?,
        None => origin::install(cfg, &args.name, None)?,
    }
    Ok(true)
}

fn cmd_uninstall(args: UninstallArgs) -> Result<bool> {
    let (origin_name, item) = origin::split_qualified(&args.name)?;
    origin::uninstall(origin_name, item)?;
    Ok(true)
}
