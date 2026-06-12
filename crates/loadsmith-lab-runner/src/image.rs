use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use loadsmith_lab_docker::{
    build_dir_context_tar, build_source_context_tar, ContainerConfig, DockerClient,
};

use crate::case::Case;
use crate::runner::RunOpts;

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
/// override binary must be runnable in the container — built for linux with a
/// glibc ≤ the image's; a host `cargo build` on a very new distro may be too
/// new. Build it in a bookworm-ish env if so.)
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
            })
            .await?;
        let code = docker.wait_container(&id).await?;
        let _ = docker.stop_and_remove(&id).await;
        anyhow::ensure!(code == 0, "`loadsmith plugin install --all` failed (exit {code})");
    }

    if overrides.is_empty() {
        return Ok(base);
    }

    // Each override is a binary or a project dir (built in-container); resolve to
    // a flat list of binaries, then layer them over the cache in a per-run overlay.
    let binaries = resolve_override_binaries(docker, overrides).await?;
    let overlay = std::env::temp_dir().join(format!("loadsmith-lab-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&overlay);
    std::fs::create_dir_all(&overlay)?;
    for entry in std::fs::read_dir(&base)? {
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
    Ok(overlay)
}

/// Resolve each `--plugin` override (a binary, a plugin crate, or a workspace
/// root) to a flat list of plugin binary paths, building any project dirs.
async fn resolve_override_binaries(
    docker: &DockerClient,
    overrides: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in overrides {
        if path.is_dir() {
            out.extend(build_plugin_project(docker, path).await?);
        } else {
            out.push(path.clone());
        }
    }
    Ok(out)
}

/// Build a plugin project in a `rust:bookworm` container (so the binaries are
/// glibc-compatible with the loadsmith image) and return host paths to the
/// produced `loadsmith-*` binaries. `path` may be a single plugin crate or a
/// workspace root (a virtual manifest → builds every member). A fresh target dir
/// is used so only the just-built binaries appear.
async fn build_plugin_project(docker: &DockerClient, path: &Path) -> Result<Vec<PathBuf>> {
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

    let target = std::env::temp_dir().join(format!("loadsmith-lab-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target)?;

    // run_container won't pull on its own (unlike build); ensure the toolchain
    // image is present.
    if !docker.image_exists("rust:bookworm").await? {
        docker.pull_image("rust:bookworm").await.context("pulling rust:bookworm")?;
    }

    tracing::info!("building plugin project {} in rust:bookworm", path.display());
    let id = docker
        .run_container(ContainerConfig {
            image: "rust:bookworm".to_string(),
            hostname: None,
            network: None,
            env: vec![("CARGO_TARGET_DIR".to_string(), "/target".to_string())],
            binds: vec![
                format!("{}:/src:ro", root.display()),
                format!("{}:/target", target.display()),
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
        })
        .await?;
    let code = docker.wait_container(&id).await?;
    let _ = docker.stop_and_remove(&id).await;
    anyhow::ensure!(code == 0, "building plugin project {} failed (exit {code})", path.display());

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

fn plugin_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".cache/loadsmith-lab/plugins"))
}

fn dir_is_empty(dir: &Path) -> Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_none())
}

/// Resolves the loadsmith image to run and returns its tag.
///
/// - `--loadsmith <dir>` (a project): build `loadsmith:local` from its Dockerfile.
/// - `--loadsmith <file>` (a binary): wrap it in a minimal image (the binary must
///   be runnable in `debian:bookworm-slim` — linux, glibc ≤ the base's).
/// - neither: use a published image (`case.loadsmith.image`, or `loadsmith:<tag>`
///   when `--tag` is given), pulling it if not already present.
pub async fn resolve_loadsmith_image(
    docker: &DockerClient,
    opts: &RunOpts,
    case: &Case,
) -> Result<String> {
    match &opts.loadsmith_source {
        Some(path) if path.is_dir() => {
            let src = path.canonicalize().unwrap_or_else(|_| path.clone());
            tracing::debug!("building loadsmith:local from project {}", src.display());
            let tar = build_source_context_tar(&src, &["target", ".git", "docs", "definitions"])?;
            docker.build_image(tar, "loadsmith:local").await?;
            Ok("loadsmith:local".to_string())
        }
        Some(path) => {
            tracing::debug!("wrapping loadsmith binary {} in an image", path.display());
            wrap_binary_image(docker, path, "loadsmith:local").await?;
            Ok("loadsmith:local".to_string())
        }
        None => {
            let image = match &opts.loadsmith_tag {
                Some(tag) => format!("loadsmith:{tag}"),
                None => case.loadsmith.image.clone(),
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
