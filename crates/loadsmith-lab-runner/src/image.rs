use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use loadsmith_lab_docker::{
    build_dir_context_tar, build_source_context_tar, ContainerConfig, DockerClient,
};

use crate::case::Case;
use crate::runner::RunOpts;

/// The canonical published loadsmith core image. The lab pulls this by default —
/// like the canonical plugin index and the content origins, the package is
/// fixed; `--tag` picks the version, a case may override the full ref, else the
/// rolling `slim` variant is used.
const LOADSMITH_IMAGE: &str = "ghcr.io/loadsmith-el/loadsmith";
const LOADSMITH_DEFAULT_TAG: &str = "slim";

/// The per-run plugin overlay (cache + `--plugin` overrides). Built once per lab
/// process — every case in a run shares it — and removed by [`remove_temp_dirs`].
/// A failed build is memoized too (as its message): every case of the run would
/// fail the same way, so retrying per case only multiplies the wait.
static OVERLAY: tokio::sync::OnceCell<Result<PathBuf, String>> = tokio::sync::OnceCell::const_new();

/// The `--loadsmith <dir|binary>` core image, built once per lab process: the
/// source can't change mid-run, and re-sending the build context per case cost
/// ~40s each even on a full cache hit.
static LOCAL_CORE: tokio::sync::OnceCell<Result<String, String>> = tokio::sync::OnceCell::const_new();

/// `uid:gid` of the lab process. Containers that write into host bind mounts run
/// as this user so everything they create is owned by the host user (root-owned
/// leftovers in /tmp or the plugin cache can't be removed without sudo).
pub(crate) fn host_user() -> Option<String> {
    #[cfg(unix)]
    {
        // SAFETY: getuid/getgid have no preconditions and cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Some(format!("{uid}:{gid}"))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn overlay_dir() -> PathBuf {
    std::env::temp_dir().join(format!("loadsmith-lab-plugins-{}", std::process::id()))
}

fn build_dir() -> PathBuf {
    std::env::temp_dir().join(format!("loadsmith-lab-build-{}", std::process::id()))
}

/// Removes this process's temp dirs (plugin overlay, plugin build target). Call
/// once at the end of a lab run.
pub fn remove_temp_dirs() {
    let _ = std::fs::remove_dir_all(overlay_dir());
    let _ = std::fs::remove_dir_all(build_dir());
}

/// Prepares the host plugin directory mounted into the loadsmith container.
///
/// Plugins are no longer bundled in the loadsmith image — they live in
/// `loadsmith-canonical-plugins` and are installed on demand. The lab keeps a
/// persistent cache (`~/.cache/loadsmith-lab/plugins`), populated once by running
/// `loadsmith plugin install --all` inside a throwaway `image` container.
///
/// `overrides` are paths to locally-built plugin binaries (the lab's `--plugin`
/// flag) — layered on top of the cache in a per-run overlay so you can test a
/// plugin you're developing against released versions of the rest. (Caveat: an
/// override *binary* must be runnable in the container. A host `cargo build`
/// links your distro's glibc, which may be newer than the image's; pass the
/// crate dir instead, which builds a static musl binary like a release does.)
pub async fn prepare_plugin_dir(
    docker: &DockerClient,
    image: &str,
    overrides: &[PathBuf],
) -> Result<PathBuf> {
    let base = plugin_cache_dir()?;
    std::fs::create_dir_all(&base)?;
    if dir_is_empty(&base)? {
        tracing::info!("populating plugin cache via `loadsmith plugin install --all`");
        let id = docker
            .run_container(ContainerConfig {
                image: image.to_string(),
                hostname: None,
                network: None,
                env: vec![],
                binds: vec![format!("{}:/plugins", base.display())],
                publish_ports: vec![],
                cmd: vec![
                    "plugin".into(),
                    "install".into(),
                    "--all".into(),
                    "--plugin-dir".into(),
                    "/plugins".into(),
                ],
                docker_args: vec![],
                user: host_user(),
            })
            .await?;
        let code = docker.wait_container(&id).await?;
        let _ = docker.stop_and_remove(&id).await;
        anyhow::ensure!(code == 0, "`loadsmith plugin install --all` failed (exit {code})");
    }

    if overrides.is_empty() {
        return Ok(base);
    }

    // The overrides are the same for every case of a run: build them once.
    OVERLAY
        .get_or_init(|| async { build_overlay(docker, &base, overrides).await.map_err(|e| format!("{e:#}")) })
        .await
        .clone()
        .map_err(anyhow::Error::msg)
}

/// Resolves each override to binaries (building project dirs in-container) and
/// layers them over the cache in this process's overlay dir.
async fn build_overlay(docker: &DockerClient, base: &Path, overrides: &[PathBuf]) -> Result<PathBuf> {
    let binaries = resolve_override_binaries(docker, overrides).await?;
    let overlay = overlay_dir();
    let _ = std::fs::remove_dir_all(&overlay);
    std::fs::create_dir_all(&overlay)?;
    for entry in std::fs::read_dir(base)? {
        let entry = entry?;
        std::fs::copy(entry.path(), overlay.join(entry.file_name()))?;
    }
    for path in &binaries {
        let name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("plugin binary {} has no filename", path.display()))?;
        let dest = overlay.join(name);
        std::fs::copy(path, &dest).with_context(|| format!("copying override {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
        tracing::info!("overriding plugin with local {}", name.to_string_lossy());
    }
    // The binaries are copied into the overlay; the build target (~1GB) is done.
    let _ = std::fs::remove_dir_all(build_dir());
    Ok(overlay)
}

/// Resolve each `--plugin` override (a binary, a plugin crate, or a workspace
/// root) to a flat list of plugin binary paths, building any project dirs.
async fn resolve_override_binaries(
    docker: &DockerClient,
    overrides: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for (i, path) in overrides.iter().enumerate() {
        if path.is_dir() {
            // One target dir per project: each build starts from an empty one,
            // so a shared dir would delete the previous project's binaries.
            let target = build_dir().join(i.to_string());
            out.extend(build_plugin_project(docker, path, &target).await?);
        } else {
            out.push(path.clone());
        }
    }
    Ok(out)
}

/// Toolchain image for `--plugin <project>` builds. Its host target is
/// `*-unknown-linux-musl`, matching the canonical release workflow.
const PLUGIN_TOOLCHAIN: &str = "rust:1-alpine";

/// Build a plugin project in a `rust:1-alpine` container (static musl binaries,
/// same as a canonical release, so they run in any loadsmith image) and return
/// host paths to the produced `loadsmith-*` binaries. `path` may be a single
/// plugin crate or a workspace root (a virtual manifest → builds every member).
/// `target` is emptied first so only the just-built binaries appear.
async fn build_plugin_project(
    docker: &DockerClient,
    path: &Path,
    target: &Path,
) -> Result<Vec<PathBuf>> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving plugin project {}", path.display()))?;
    let root = workspace_root(&path);
    let rel = path.strip_prefix(&root).unwrap_or(Path::new(""));
    let manifest = if rel.as_os_str().is_empty() {
        "/src/Cargo.toml".to_string()
    } else {
        format!("/src/{}/Cargo.toml", rel.display())
    };

    let _ = std::fs::remove_dir_all(target);
    std::fs::create_dir_all(target)?;
    // Running as the host user, cargo can't write the image's root-owned
    // CARGO_HOME; give it a persistent user-owned one (also caches the registry
    // across runs).
    let cargo_home = lab_cache_dir()?.join("cargo-home");
    std::fs::create_dir_all(&cargo_home)?;

    // run_container won't pull on its own (unlike build); ensure the toolchain
    // image is present.
    if !docker.image_exists(PLUGIN_TOOLCHAIN).await? {
        docker
            .pull_image(PLUGIN_TOOLCHAIN)
            .await
            .with_context(|| format!("pulling {PLUGIN_TOOLCHAIN}"))?;
    }

    tracing::info!("building plugin project {} in {PLUGIN_TOOLCHAIN}", path.display());
    let id = docker
        .run_container(ContainerConfig {
            image: PLUGIN_TOOLCHAIN.to_string(),
            hostname: None,
            network: None,
            env: vec![
                ("CARGO_TARGET_DIR".to_string(), "/target".to_string()),
                ("CARGO_HOME".to_string(), "/cargo-home".to_string()),
            ],
            binds: vec![
                format!("{}:/src:ro", root.display()),
                format!("{}:/target", target.display()),
                format!("{}:/cargo-home", cargo_home.display()),
            ],
            publish_ports: vec![],
            cmd: vec![
                "cargo".into(),
                "build".into(),
                "--release".into(),
                "--manifest-path".into(),
                manifest,
            ],
            docker_args: vec![],
            user: host_user(),
        })
        .await?;
    let code = docker.wait_container(&id).await?;
    // On failure, surface cargo's own error — otherwise all you get is "exit 101".
    let log_tail = if code == 0 {
        String::new()
    } else {
        let logs = docker.container_logs(&id).await.unwrap_or_default();
        let lines: Vec<&str> = logs.lines().collect();
        let mut tail = lines[lines.len().saturating_sub(15)..].join("\n");
        // The source is mounted read-only, so cargo can't refresh a stale lockfile
        // (typically right after a version bump).
        if logs.contains("failed to write /src/Cargo.lock") {
            tail.push_str(
                "\nhint: Cargo.lock is out of date; run any cargo command (e.g. \
                 `cargo metadata`) in the project on the host to refresh it",
            );
        }
        tail
    };
    let _ = docker.stop_and_remove(&id).await;
    anyhow::ensure!(
        code == 0,
        "building plugin project {} failed (exit {code}):\n{log_tail}",
        path.display()
    );

    let release = target.join("release");
    let mut bins = Vec::new();
    for entry in std::fs::read_dir(&release)
        .with_context(|| format!("reading {}", release.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Built plugin binaries are `loadsmith-*` executables (skip `.d` deps).
        if name.starts_with("loadsmith-") && is_executable(&entry.path()) {
            bins.push(entry.path());
        }
    }
    anyhow::ensure!(
        !bins.is_empty(),
        "no loadsmith-* binaries produced building {}",
        path.display()
    );
    Ok(bins)
}

/// The Cargo workspace root containing `start` (walks up for a `Cargo.toml` with
/// a `[workspace]` table); falls back to `start` itself (a standalone crate).
fn workspace_root(start: &Path) -> PathBuf {
    let mut dir = if start.is_dir() {
        Some(start.to_path_buf())
    } else {
        start.parent().map(Path::to_path_buf)
    };
    while let Some(d) = dir {
        let cargo = d.join("Cargo.toml");
        if cargo.is_file() && std::fs::read_to_string(&cargo).unwrap_or_default().contains("[workspace]") {
            return d;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    start.to_path_buf()
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn lab_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".cache/loadsmith-lab"))
}

fn plugin_cache_dir() -> Result<PathBuf> {
    Ok(lab_cache_dir()?.join("plugins"))
}

fn dir_is_empty(dir: &Path) -> Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_none())
}

/// Resolves the loadsmith image to run and returns its tag.
///
/// - `--loadsmith <dir>` (a project): build `loadsmith:local` from its Dockerfile.
/// - `--loadsmith <file>` (a binary): wrap it in a minimal image (the binary must
///   be runnable in `debian:bookworm-slim` — linux, glibc ≤ the base's).
/// - neither: pull the canonical published image `ghcr.io/loadsmith-el/loadsmith`
///   — `--tag <t>` picks the version, else a case's explicit `loadsmith.image`,
///   else the rolling `:slim` variant. Pulled if not already present.
pub async fn resolve_loadsmith_image(
    docker: &DockerClient,
    opts: &RunOpts,
    case: &Case,
) -> Result<String> {
    match &opts.loadsmith_source {
        Some(path) => LOCAL_CORE
            .get_or_init(|| async { build_local_core(docker, path).await.map_err(|e| format!("{e:#}")) })
            .await
            .clone()
            .map_err(anyhow::Error::msg),
        None => {
            let image = match (&opts.loadsmith_tag, &case.loadsmith.image) {
                (Some(tag), _) => format!("{LOADSMITH_IMAGE}:{tag}"),
                (None, Some(img)) => img.clone(),
                (None, None) => format!("{LOADSMITH_IMAGE}:{LOADSMITH_DEFAULT_TAG}"),
            };
            if docker.image_exists(&image).await? {
                return Ok(image);
            }
            match docker.pull_image(&image).await {
                Ok(()) => Ok(image),
                Err(e) => anyhow::bail!(
                    "loadsmith image '{image}' not found locally and pull failed: {e}\n\
                     hint: pass --loadsmith <binary | project dir> to run a local core, \
                     or publish/tag the image first"
                ),
            }
        }
    }
}

/// Builds `loadsmith:local` from a project dir (its Dockerfile) or by wrapping a binary.
async fn build_local_core(docker: &DockerClient, path: &Path) -> Result<String> {
    if path.is_dir() {
        let src = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        tracing::debug!("building loadsmith:local from project {}", src.display());
        let tar = build_source_context_tar(&src, &["target", ".git", "docs", "definitions"])?;
        docker.build_image(tar, "loadsmith:local").await?;
    } else {
        tracing::debug!("wrapping loadsmith binary {} in an image", path.display());
        wrap_binary_image(docker, path, "loadsmith:local").await?;
    }
    Ok("loadsmith:local".to_string())
}

/// Wraps a prebuilt loadsmith binary in a minimal runnable image. ca-certs are
/// included because the lab runs `loadsmith plugin install --all` with this
/// image to populate the plugin cache.
async fn wrap_binary_image(docker: &DockerClient, binary: &Path, tag: &str) -> Result<()> {
    let ctx = std::env::temp_dir().join(format!("loadsmith-wrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ctx);
    std::fs::create_dir_all(&ctx)?;
    std::fs::copy(binary, ctx.join("loadsmith"))
        .with_context(|| format!("copying loadsmith binary {}", binary.display()))?;
    std::fs::write(
        ctx.join("Dockerfile"),
        "FROM debian:bookworm-slim\n\
         RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
             && rm -rf /var/lib/apt/lists/*\n\
         COPY loadsmith /usr/local/bin/loadsmith\n\
         RUN chmod 755 /usr/local/bin/loadsmith\n\
         ENTRYPOINT [\"loadsmith\"]\n",
    )?;
    let tar = build_dir_context_tar(&ctx, &[])?;
    docker.build_image(tar, tag).await?;
    let _ = std::fs::remove_dir_all(&ctx);
    Ok(())
}

/// Resolves an image to the tag `image`: local → pull → build from
/// `context_dir`'s Dockerfile → error. Returns `true` if it had to be built.
///
/// `context_dir` is a service-image directory (`Dockerfile` + init files)
/// resolved from an image origin — see [`crate::origin`]. The image is
/// responsible for its own seed data: the canonical postgres image, for
/// example, clones `loadsmith-lab-canonical-data` and generates the CSV in a
/// build stage, so the lab only needs to tar the directory.
///
/// The pull step keeps the future registry path alive (a published tag is used
/// as-is); local-only tags simply fail the pull and fall through to a build.
// Future registry hook: when images are published, pull a registry-qualified
// tag (`<registry>/<origin>/<name>:<tag>`) instead of building — gated on a
// config value that doesn't exist yet, so no code change here for now.
pub async fn resolve_image(
    docker: &DockerClient,
    image: &str,
    context_dir: &Path,
) -> Result<bool> {
    if docker.image_exists(image).await? {
        tracing::debug!("image {image} found locally");
        return Ok(false);
    }
    tracing::debug!("image {image} not found locally, trying pull...");
    match docker.pull_image(image).await {
        Ok(()) => {
            tracing::debug!("pulled {image}");
            return Ok(false);
        }
        Err(e) => tracing::debug!("pull failed: {e}"),
    }
    anyhow::ensure!(
        context_dir.join("Dockerfile").is_file(),
        "image {image} not found locally or in registry, and no Dockerfile found at {}",
        context_dir.display()
    );
    tracing::debug!("building {image} from {}", context_dir.display());
    let tar = build_dir_context_tar(context_dir, &[])?;
    docker.build_image(tar, image).await?;
    tracing::debug!("built {image}");
    Ok(true)
}
