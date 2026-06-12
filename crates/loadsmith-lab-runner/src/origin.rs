//! Origins: where cases, bundles, and service images come from.
//!
//! The lab engine itself ships no content. Instead it resolves cases/bundles/
//! images from registered **origins**, each addressed as `<origin>/<name>`:
//!
//! - **Remote origin** (git): registered with a URL, shallow-cloned into the
//!   cache dir. Its content is not usable until `install`ed into the workdir.
//! - **Local origin** (path): registered with a filesystem path, read live in
//!   place — never cloned, copied, or installed. The dev workflow.
//!
//! Every origin has a `loadsmith-lab.toml` manifest at its root: a minimal
//! index of names + descriptions per category. It doesn't redefine paths — an
//! entry `cases.postgres-to-jsonl` always lives at `cases/postgres-to-jsonl/`.
//!
//! See `docs/src/architecture/overview.md` for the full model.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The three kinds of content an origin can provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cases,
    Bundles,
    Images,
}

impl Kind {
    /// The content subdirectory name at an origin root (and in the workdir).
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Cases => "cases",
            Kind::Bundles => "bundles",
            Kind::Images => "images",
        }
    }

    /// The manifest file inside an item directory, if any (images are just a
    /// Dockerfile dir, so they have none).
    fn item_manifest(self) -> Option<&'static str> {
        match self {
            Kind::Cases => Some("case.yaml"),
            Kind::Bundles => Some("bundle.yaml"),
            Kind::Images => None,
        }
    }

    fn all() -> [Kind; 3] {
        [Kind::Cases, Kind::Bundles, Kind::Images]
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir())
    }
}

// --- config (origins.toml) -------------------------------------------------

/// One registered origin. Serialized verbatim to `origins.toml`:
/// ```toml
/// [[origins]]
/// name = "catalog"
/// source = "git"      # or "path"
/// location = "https://github.com/loadsmith-el/loadsmith-lab-catalog.git"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginConfig {
    pub name: String,
    /// "git" (remote) or "path" (local).
    pub source: String,
    /// Clone URL for git origins, filesystem path for path origins.
    pub location: String,
}

impl OriginConfig {
    pub fn is_git(&self) -> bool {
        self.source == "git"
    }
    pub fn is_path(&self) -> bool {
        self.source == "path"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, rename = "origins")]
    pub origins: Vec<OriginConfig>,
}

impl Config {
    pub fn find(&self, name: &str) -> Option<&OriginConfig> {
        self.origins.iter().find(|o| o.name == name)
    }
    pub fn git_origins(&self) -> impl Iterator<Item = &OriginConfig> {
        self.origins.iter().filter(|o| o.is_git())
    }
    pub fn path_origins(&self) -> impl Iterator<Item = &OriginConfig> {
        self.origins.iter().filter(|o| o.is_path())
    }
}

/// The default origins seeded on first run (not auto-cloned — lazily fetched
/// the first time their content/manifest is needed).
fn default_config() -> Config {
    Config {
        origins: vec![
            OriginConfig {
                name: "catalog".into(),
                source: "git".into(),
                location: "https://github.com/loadsmith-el/loadsmith-lab-catalog.git".into(),
            },
            OriginConfig {
                name: "images".into(),
                source: "git".into(),
                location: "https://github.com/loadsmith-el/loadsmith-lab-images.git".into(),
            },
        ],
    }
}

fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("no config dir available on this platform")?
        .join("loadsmith-lab"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("origins.toml"))
}

/// Loads `origins.toml`, writing the default `catalog`/`images` entries if it
/// doesn't exist yet.
pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        let cfg = default_config();
        save_config(&cfg)?;
        return Ok(cfg);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save_config(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// --- filesystem locations --------------------------------------------------

/// Where `install` copies remote-origin content to. Items land at
/// `<workdir>/{cases,bundles,images}/<origin>/<name>/`.
pub fn workdir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("no data dir available on this platform")?
        .join("loadsmith-lab/installed"))
}

/// The cache clone location for a git origin.
pub fn origin_cache_dir(name: &str) -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("no cache dir available on this platform")?
        .join("loadsmith-lab/origins")
        .join(name))
}

/// Resolves an origin's root directory (where `loadsmith-lab.toml` and the
/// `cases/`/`bundles/`/`images/` dirs live).
///
/// - path origin → the registered path (must exist).
/// - git origin → the cache clone. When `ensure` is true and the clone is
///   missing, it is cloned first (the lazy-clone path).
pub fn origin_root(origin: &OriginConfig, ensure: bool) -> Result<PathBuf> {
    if origin.is_path() {
        let root = PathBuf::from(&origin.location);
        anyhow::ensure!(
            root.exists(),
            "local origin '{}' path does not exist: {}",
            origin.name,
            root.display()
        );
        return Ok(root);
    }
    let cache = origin_cache_dir(&origin.name)?;
    if !cache.exists() {
        anyhow::ensure!(
            ensure,
            "remote origin '{}' is not cloned yet — run a command that fetches it \
             (e.g. `loadsmith-lab origin show {}`)",
            origin.name,
            origin.name
        );
        eprintln!("cloning origin '{}' from {}…", origin.name, origin.location);
        clone_to_cache(&origin.location, &cache)?;
    }
    Ok(cache)
}

// --- git operations (shell-out, like cmd_generate's python3) ---------------

pub fn clone_to_cache(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(dest)
        .status()
        .context("failed to run git (is it installed?)")?;
    anyhow::ensure!(status.success(), "git clone of {url} failed");
    Ok(())
}

/// `git pull` (or initial clone) for one git origin to refresh its cache clone.
pub fn update_origin(origin: &OriginConfig) -> Result<()> {
    anyhow::ensure!(
        origin.is_git(),
        "origin '{}' is a local origin — nothing to update (it's read live)",
        origin.name
    );
    let cache = origin_cache_dir(&origin.name)?;
    if !cache.exists() {
        eprintln!("cloning origin '{}' from {}…", origin.name, origin.location);
        return clone_to_cache(&origin.location, &cache);
    }
    let status = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(&cache)
        .status()
        .context("failed to run git")?;
    anyhow::ensure!(status.success(), "git pull for origin '{}' failed", origin.name);
    Ok(())
}

/// Lightweight online check: does the remote have commits the local clone
/// doesn't? Uses `git ls-remote` (refs only, no object transfer). Returns
/// `Ok(false)` — never an error — if offline or not yet cloned, so callers can
/// treat a connectivity failure as "no hint".
pub fn check_for_update(origin: &OriginConfig) -> Result<bool> {
    if !origin.is_git() {
        return Ok(false);
    }
    let cache = origin_cache_dir(&origin.name)?;
    if !cache.exists() {
        return Ok(false);
    }
    let local = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cache)
        .output();
    let Ok(local) = local else { return Ok(false) };
    if !local.status.success() {
        return Ok(false);
    }
    let local_sha = String::from_utf8_lossy(&local.stdout).trim().to_string();

    let remote = Command::new("git")
        .args(["ls-remote", &origin.location, "HEAD"])
        .output();
    let Ok(remote) = remote else { return Ok(false) };
    if !remote.status.success() {
        return Ok(false);
    }
    let remote_sha = String::from_utf8_lossy(&remote.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if remote_sha.is_empty() {
        return Ok(false);
    }
    Ok(local_sha != remote_sha)
}

/// One "new version available" line per git origin that has remote changes.
pub fn update_hints(cfg: &Config) -> Vec<String> {
    cfg.git_origins()
        .filter(|o| check_for_update(o).unwrap_or(false))
        .map(|o| {
            format!(
                "origin '{}': new version available — run 'loadsmith-lab origin remote update {}' to refresh",
                o.name, o.name
            )
        })
        .collect()
}

// --- manifest --------------------------------------------------------------

/// The `loadsmith-lab.toml` index at an origin root: name → description per
/// category. Absent categories deserialize as empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub cases: BTreeMap<String, String>,
    #[serde(default)]
    pub bundles: BTreeMap<String, String>,
    #[serde(default)]
    pub images: BTreeMap<String, String>,
}

impl Manifest {
    pub fn category(&self, kind: Kind) -> &BTreeMap<String, String> {
        match kind {
            Kind::Cases => &self.cases,
            Kind::Bundles => &self.bundles,
            Kind::Images => &self.images,
        }
    }
}

pub const MANIFEST_FILE: &str = "loadsmith-lab.toml";

pub fn read_manifest(origin_root: &Path) -> Result<Manifest> {
    let path = origin_root.join(MANIFEST_FILE);
    anyhow::ensure!(
        path.exists(),
        "no {MANIFEST_FILE} at {} — not a valid origin",
        origin_root.display()
    );
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Reads the manifest for a named origin (lazily cloning a git origin).
pub fn manifest_of(cfg: &Config, origin_name: &str) -> Result<Manifest> {
    let origin = cfg
        .find(origin_name)
        .with_context(|| format!("no such origin: '{origin_name}'"))?;
    let root = origin_root(origin, true)?;
    read_manifest(&root)
}

// --- install / uninstall ---------------------------------------------------

/// Copies content from a git origin's cache clone into the workdir.
///
/// `item = Some(name)` installs that one item (looked up in the manifest to
/// find its category); `item = None` installs everything in the manifest.
/// Installing a bundle also installs (recursively, across origins) every case
/// it references in `bundle.yaml`.
pub fn install(cfg: &Config, origin_name: &str, item: Option<&str>) -> Result<()> {
    let mut seen = HashSet::new();
    install_inner(cfg, origin_name, item, &mut seen)
}

/// Recursive worker for [`install`]. `seen` tracks `<origin>/<name>` keys
/// already copied in this install run, so a case pulled in both directly and
/// as a bundle dependency is only copied (and printed) once, and bundles
/// referencing each other can't recurse forever.
fn install_inner(
    cfg: &Config,
    origin_name: &str,
    item: Option<&str>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    let origin = cfg
        .find(origin_name)
        .with_context(|| format!("no such origin: '{origin_name}'"))?;
    anyhow::ensure!(
        origin.is_git(),
        "origin '{origin_name}' is a local origin — its content is used live, no install needed"
    );
    let root = origin_root(origin, true)?;
    let manifest = read_manifest(&root)?;
    let workdir = workdir()?;

    let mut installed = 0usize;
    for kind in Kind::all() {
        for name in manifest.category(kind).keys() {
            if let Some(want) = item {
                if want != name {
                    continue;
                }
            }
            installed += 1;

            let key = format!("{origin_name}/{name}");
            if !seen.insert(key) {
                continue;
            }

            let src = root.join(kind.dir()).join(name);
            anyhow::ensure!(
                src.exists(),
                "manifest lists {kind}.{name} but {} is missing in origin '{origin_name}'",
                src.display()
            );
            let dest = workdir.join(kind.dir()).join(origin_name).join(name);
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            copy_dir_all(&src, &dest)?;
            println!("installed {origin_name}/{name} ({kind})");

            if kind == Kind::Bundles {
                install_bundle_deps(cfg, &dest, seen)?;
            }
        }
    }

    if let Some(want) = item {
        anyhow::ensure!(
            installed > 0,
            "origin '{origin_name}' has no item named '{want}' in its manifest"
        );
    } else {
        anyhow::ensure!(installed > 0, "origin '{origin_name}' manifest is empty");
    }
    Ok(())
}

/// Installs every case a just-installed bundle references in `bundle.yaml`,
/// recursively. Cases in local (path) origins are skipped — they're read live
/// and never installed. References to unregistered origins are a hard error:
/// a config problem, not something to silently skip.
fn install_bundle_deps(cfg: &Config, bundle_dir: &Path, seen: &mut HashSet<String>) -> Result<()> {
    let bundle = crate::bundle::load_bundle(&bundle_dir.join("bundle.yaml"))?;
    for entry in &bundle.cases {
        let (case_origin, case_name) = split_qualified(&entry.case)?;
        let origin = cfg.find(case_origin).with_context(|| {
            format!(
                "bundle '{}' references case '{}' from unregistered origin '{case_origin}'",
                bundle.bundle.name, entry.case
            )
        })?;
        if origin.is_path() {
            continue;
        }
        install_inner(cfg, case_origin, Some(case_name), seen)?;
    }
    Ok(())
}

/// Removes an installed `<origin>/<name>` from the workdir (searching all
/// categories). Errors if nothing was installed under that name.
pub fn uninstall(origin_name: &str, item: &str) -> Result<()> {
    let workdir = workdir()?;
    let mut removed = false;
    for kind in Kind::all() {
        let dest = workdir.join(kind.dir()).join(origin_name).join(item);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
            println!("uninstalled {origin_name}/{item} ({kind})");
            removed = true;
        }
    }
    anyhow::ensure!(removed, "{origin_name}/{item} is not installed");
    Ok(())
}

// --- resolution & discovery ------------------------------------------------

/// Splits `<origin>/<name>` into its two parts.
pub fn split_qualified(qualified: &str) -> Result<(&str, &str)> {
    qualified.split_once('/').with_context(|| {
        format!("expected an <origin>/<name> reference, got '{qualified}' (names are always namespaced)")
    })
}

/// Resolves a `<origin>/<name>` reference to its item directory on disk.
///
/// - local origin → the live path under the registered directory.
/// - git origin → the installed copy in the workdir.
///
/// Errors with an install hint when a git origin's item isn't installed yet.
pub fn resolve_item(cfg: &Config, qualified: &str, kind: Kind) -> Result<PathBuf> {
    let (origin_name, name) = split_qualified(qualified)?;
    let origin = cfg
        .find(origin_name)
        .with_context(|| format!("no such origin: '{origin_name}' (see `loadsmith-lab origin list`)"))?;

    let dir = if origin.is_path() {
        let root = origin_root(origin, false)?;
        let dir = root.join(kind.dir()).join(name);
        anyhow::ensure!(
            item_present(&dir, kind),
            "{kind} '{qualified}' not found at {} (local origin)",
            dir.display()
        );
        dir
    } else {
        let dir = workdir()?.join(kind.dir()).join(origin_name).join(name);
        anyhow::ensure!(
            item_present(&dir, kind),
            "{kind} '{qualified}' is not installed — run `loadsmith-lab install {qualified}`"
        );
        dir
    };
    Ok(dir)
}

/// True if `dir` holds an item of `kind` (its manifest file exists, or for
/// images the dir holds a Dockerfile).
fn item_present(dir: &Path, kind: Kind) -> bool {
    match kind.item_manifest() {
        Some(file) => dir.join(file).exists(),
        None => dir.join("Dockerfile").exists(),
    }
}

/// All installed (workdir) + live (local origin) items of a kind, each as
/// `(<origin>/<name>, item_dir)`, sorted by qualified name.
pub fn discover(cfg: &Config, kind: Kind) -> Result<Vec<(String, PathBuf)>> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();

    // Installed copies: <workdir>/<kind>/<origin>/<name>/
    let installed_root = workdir()?.join(kind.dir());
    if let Ok(origins) = std::fs::read_dir(&installed_root) {
        for origin_entry in origins.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
            let origin_name = origin_entry.file_name().to_string_lossy().to_string();
            collect_items(&origin_entry.path(), &origin_name, kind, &mut out);
        }
    }

    // Live local origins: <path>/<kind>/<name>/
    for origin in cfg.path_origins() {
        let Ok(root) = origin_root(origin, false) else { continue };
        let dir = root.join(kind.dir());
        collect_items(&dir, &origin.name, kind, &mut out);
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Pushes every valid `kind` item directly under `parent` as
/// `(<origin>/<name>, dir)`.
fn collect_items(parent: &Path, origin_name: &str, kind: Kind, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
        let dir = entry.path();
        if item_present(&dir, kind) {
            let name = entry.file_name().to_string_lossy().to_string();
            out.push((format!("{origin_name}/{name}"), dir));
        }
    }
}

/// Items offered by remote (git) origins' manifests that aren't installed yet,
/// as `(<origin>/<name>, description)`. For `list --available`.
pub fn available(cfg: &Config, kind: Kind) -> Result<Vec<(String, String)>> {
    let workdir = workdir()?;
    let mut out = Vec::new();
    for origin in cfg.git_origins() {
        // Only consult an already-cloned origin; don't force a network fetch
        // just to list. (A freshly seeded default origin shows nothing until
        // its first explicit use.)
        let Ok(root) = origin_root(origin, false) else { continue };
        let Ok(manifest) = read_manifest(&root) else { continue };
        for (name, desc) in manifest.category(kind) {
            let installed = workdir.join(kind.dir()).join(&origin.name).join(name);
            if !installed.exists() {
                out.push((format!("{}/{}", origin.name, name), desc.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// --- helpers ---------------------------------------------------------------

/// Recursively copies a directory tree (files + subdirs; symlinks skipped).
fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Derives the local Docker build tag for an `<origin>/<name>` image item.
///
/// Slash-separated so it lines up with a future registry-qualified tag
/// (`<registry>/<origin>/<name>:<tag>`) once images are published via CI — for
/// now everything is built locally under the `:local` tag.
pub fn image_tag(qualified: &str) -> String {
    format!("loadsmith-lab/{qualified}:local")
}
