use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use bollard::Docker;

pub struct DockerClient {
    inner: Docker,
}

#[derive(Debug, Default)]
pub struct ContainerConfig {
    pub image: String,
    pub hostname: Option<String>,
    pub network: Option<String>,
    pub env: Vec<(String, String)>,
    pub binds: Vec<String>,
    pub publish_ports: Vec<(u16, u16)>,
    /// Command to run in the container. Empty uses the image's default
    /// CMD/ENTRYPOINT; non-empty overrides the CMD (appended to ENTRYPOINT).
    pub cmd: Vec<String>,
    pub docker_args: Vec<String>,
}

impl DockerClient {
    pub async fn new() -> Result<Self> {
        let inner = Docker::connect_with_local_defaults()?;
        Ok(Self { inner })
    }

    pub async fn image_exists(&self, name: &str) -> Result<bool> {
        match self.inner.inspect_image(name).await {
            Ok(_) => Ok(true),
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn pull_image(&self, name: &str) -> Result<()> {
        use bollard::image::CreateImageOptions;
        use futures_util::TryStreamExt;

        let (image, tag) = name.rsplit_once(':').unwrap_or((name, "latest"));
        let mut stream = self.inner.create_image(
            Some(CreateImageOptions { from_image: image, tag, ..Default::default() }),
            None,
            None,
        );
        while let Some(info) = stream.try_next().await? {
            if let Some(status) = info.status {
                tracing::debug!("pull {name}: {status}");
            }
        }
        Ok(())
    }

    pub async fn build_image(&self, context_tar: Vec<u8>, tag: &str) -> Result<()> {
        use bollard::image::BuildImageOptions;
        use futures_util::TryStreamExt;

        let opts = BuildImageOptions { t: tag, ..Default::default() };
        let body = bytes::Bytes::from(context_tar);
        let mut stream = self.inner.build_image(opts, None, Some(body));
        while let Some(info) = stream.try_next().await? {
            if let Some(stream) = info.stream {
                let msg = stream.trim_end();
                if !msg.is_empty() {
                    tracing::debug!("build {tag}: {msg}");
                }
            }
            if let Some(err) = info.error {
                anyhow::bail!("docker build error: {err}");
            }
        }
        Ok(())
    }

    pub async fn create_network(&self, name: &str) -> Result<String> {
        use bollard::network::CreateNetworkOptions;

        let resp = self.inner.create_network(CreateNetworkOptions {
            name,
            driver: "bridge",
            ..Default::default()
        }).await?;
        Ok(resp.id.unwrap_or_else(|| name.to_string()))
    }

    pub async fn remove_network(&self, name: &str) -> Result<()> {
        self.inner.remove_network(name).await?;
        Ok(())
    }

    pub async fn run_container(&self, config: ContainerConfig) -> Result<String> {
        use bollard::container::{Config, CreateContainerOptions, StartContainerOptions};
        use bollard::models::{HostConfig, PortBinding};
        use std::collections::HashMap;

        let env: Vec<String> = config.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let env_refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();

        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
        for (host_port, container_port) in &config.publish_ports {
            let key = format!("{container_port}/tcp");
            port_bindings.insert(key.clone(), Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".into()),
                host_port: Some(host_port.to_string()),
            }]));
            exposed_ports.insert(key, HashMap::new());
        }

        let host_config = HostConfig {
            binds: if config.binds.is_empty() { None } else { Some(config.binds) },
            port_bindings: if port_bindings.is_empty() { None } else { Some(port_bindings) },
            ..Default::default()
        };

        let cmd_refs: Vec<&str> = config.cmd.iter().map(|s| s.as_str()).collect();

        let name = config.hostname.clone();
        let container_config = Config {
            image: Some(config.image.as_str()),
            hostname: config.hostname.as_deref(),
            env: if env_refs.is_empty() { None } else { Some(env_refs) },
            cmd: if cmd_refs.is_empty() { None } else { Some(cmd_refs) },
            exposed_ports: if exposed_ports.is_empty() { None } else { Some(
                exposed_ports.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()
            )},
            host_config: Some(host_config),
            ..Default::default()
        };

        let opts = name.as_deref().map(|n| CreateContainerOptions { name: n, platform: None });
        let resp = self.inner.create_container(opts, container_config).await?;

        if config.network.is_some() {
            self.inner.connect_network(
                config.network.as_deref().unwrap(),
                bollard::network::ConnectNetworkOptions {
                    container: &resp.id,
                    endpoint_config: bollard::models::EndpointSettings {
                        aliases: config.hostname.as_ref().map(|h| vec![h.clone()]),
                        ..Default::default()
                    },
                },
            ).await?;
        }

        self.inner.start_container(&resp.id, None::<StartContainerOptions<String>>).await?;
        Ok(resp.id)
    }

    pub async fn wait_container(&self, id: &str) -> Result<i64> {
        use bollard::container::WaitContainerOptions;
        use futures_util::StreamExt;

        let mut stream = self.inner.wait_container(id, None::<WaitContainerOptions<String>>);
        let mut exit_code = 0i64;
        while let Some(result) = stream.next().await {
            match result {
                Ok(r) => exit_code = r.status_code,
                // bollard surfaces a non-zero container exit as an error rather
                // than a status code; recover the code so callers see "exit N"
                // instead of a generic wait error.
                Err(bollard::errors::Error::DockerContainerWaitError { code, .. }) => {
                    exit_code = code;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(exit_code)
    }

    pub async fn stop_and_remove(&self, id: &str) -> Result<()> {
        use bollard::container::RemoveContainerOptions;

        let _ = self.inner.stop_container(id, None).await;
        self.inner.remove_container(id, Some(RemoveContainerOptions { force: true, ..Default::default() })).await?;
        Ok(())
    }

    pub async fn wait_tcp(&self, host: &str, port: u16, timeout: Duration) -> Result<()> {
        use tokio::net::TcpStream;
        use tokio::time::{sleep, Instant};

        let deadline = Instant::now() + timeout;
        let addr = format!("{host}:{port}");
        loop {
            if TcpStream::connect(&addr).await.is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for {addr} to be ready");
            }
            sleep(Duration::from_millis(500)).await;
        }
    }

    /// Returns the container's IP address on the given Docker network.
    /// Use this to probe readiness from the host when the container hostname
    /// is only resolvable inside the Docker network.
    pub async fn container_ip(&self, id: &str, net_name: &str) -> Result<String> {
        let info = self.inner.inspect_container(id, None).await?;
        let ip = info
            .network_settings
            .and_then(|ns| ns.networks)
            .and_then(|mut nets| nets.remove(net_name))
            .and_then(|ep| ep.ip_address)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("no IP for container {id} on network {net_name}"))?;
        Ok(ip)
    }

    pub async fn container_logs(&self, id: &str) -> Result<String> {
        use bollard::container::LogsOptions;
        use futures_util::TryStreamExt;

        let opts = LogsOptions::<String> { stdout: true, stderr: true, ..Default::default() };
        let chunks: Vec<_> = self.inner.logs(id, Some(opts)).try_collect().await?;
        let mut out = String::new();
        for chunk in chunks {
            out.push_str(&chunk.to_string());
        }
        Ok(out)
    }

    /// Follows the container's combined stdout+stderr in real time, invoking
    /// `on_line` for each complete line as it arrives, and returns the full
    /// accumulated text once the stream ends (i.e. when the container exits).
    ///
    /// Docker log chunks do not align with line boundaries, so partial lines are
    /// buffered until a newline arrives.
    pub async fn stream_logs<F: FnMut(&str)>(&self, id: &str, mut on_line: F) -> Result<String> {
        use bollard::container::LogsOptions;
        use futures_util::StreamExt;

        let opts = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: true,
            ..Default::default()
        };
        let mut stream = self.inner.logs(id, Some(opts));

        let mut acc = String::new();
        let mut buf = String::new();
        while let Some(item) = stream.next().await {
            let chunk = item?.to_string();
            acc.push_str(&chunk);
            buf.push_str(&chunk);
            // Emit every complete line; keep the trailing partial in `buf`.
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                on_line(line.trim_end_matches('\n'));
            }
        }
        // Flush any trailing line without a final newline.
        if !buf.is_empty() {
            on_line(buf.trim_end_matches('\n'));
        }
        Ok(acc)
    }
}

/// Builds an in-memory tar of a source tree at `root`, suitable as a Docker
/// build context. Files are stored at their path relative to `root` (so a
/// top-level `Dockerfile` and `Cargo.toml` land at the tar root). Any path whose
/// first component is in `exclude` is skipped (e.g. `target`, `.git`).
///
/// The lab sends this tar straight to bollard's classic builder, which does NOT
/// honor `.dockerignore` — exclusion must happen here.
pub fn build_source_context_tar(root: &Path, exclude: &[&str]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        root.join("Dockerfile").is_file(),
        "no Dockerfile at {} — cannot build the loadsmith image",
        root.display()
    );
    anyhow::ensure!(
        root.join("Cargo.lock").is_file(),
        "Cargo.lock not found at {} — run `cargo build` in the loadsmith repo first \
         (cargo-chef needs a lockfile for a cacheable build)",
        root.display()
    );

    let mut ar = tar::Builder::new(Vec::new());
    append_dir(&mut ar, root, root, exclude)?;
    Ok(ar.into_inner()?)
}

/// Builds an in-memory tar of a plain directory tree at `root` for use as a
/// Docker build context. Like [`build_source_context_tar`] but with no
/// Rust/cargo-chef assumptions — it only requires a `Dockerfile` at the root.
/// Used for ad-hoc images (e.g. bundle hook images) whose context is just a
/// Dockerfile plus some scripts/fixtures.
pub fn build_dir_context_tar(root: &Path, exclude: &[&str]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        root.join("Dockerfile").is_file(),
        "no Dockerfile at {} — cannot build the image",
        root.display()
    );

    let mut ar = tar::Builder::new(Vec::new());
    append_dir(&mut ar, root, root, exclude)?;
    Ok(ar.into_inner()?)
}

/// Recursively appends `dir`'s files to `ar`, naming each entry relative to
/// `root`, skipping any directory whose name is in `exclude`.
fn append_dir(
    ar: &mut tar::Builder<Vec<u8>>,
    root: &Path,
    dir: &Path,
    exclude: &[&str],
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if exclude.contains(&name.as_ref()) {
                continue;
            }
            append_dir(ar, root, &path, exclude)?;
        } else if file_type.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            ar.append_path_with_name(&path, rel)?;
        }
        // Symlinks and other entry kinds are skipped.
    }
    Ok(())
}

