// Named-tunnel configuration hook.
//
// When `~/.config/oxiremote/tunnel.toml` is present, the agent runs a
// Cloudflare Named Tunnel instead of a Quick Tunnel. Useful for production
// where you want a stable hostname and credentials committed to a cred file.
//
// TOML schema:
//   tunnel_name        = "my-tunnel"         # required
//   credentials_file   = "/path/creds.json"  # optional — cloudflared also
//                                             #   auto-discovers in the
//                                             #   default cert dir
//   hostname           = "oxi.example.com"   # optional — for logging
//
// CLI helper:
//   `oxiremote tunnel use <name>`  writes a tunnel.toml with sane defaults.

use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedTunnelConfig {
    pub tunnel_name: String,
    #[serde(default)]
    pub credentials_file: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME or USERPROFILE not set")?;
    Ok(PathBuf::from(home).join(".config").join("oxiremote").join("tunnel.toml"))
}

pub fn load() -> anyhow::Result<Option<NamedTunnelConfig>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let cfg: NamedTunnelConfig = toml::from_str(&text).context("parse tunnel.toml")?;
    Ok(Some(cfg))
}

pub fn write_default(tunnel_name: &str) -> anyhow::Result<PathBuf> {
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("create config dir")?;
    }
    let cfg = NamedTunnelConfig {
        tunnel_name: tunnel_name.to_string(),
        credentials_file: None,
        hostname: None,
    };
    let text = toml::to_string_pretty(&cfg).context("serialize tunnel.toml")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// CLI entrypoint for `oxiremote tunnel use <name>`.
pub fn cli_use(args: Vec<String>) -> anyhow::Result<()> {
    let name = args.first().context("tunnel name required")?;
    let path = write_default(name)?;
    println!("wrote {}", path.display());
    println!("run `cloudflared tunnel create {name}` and update credentials_file if needed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_config() {
        let cfg = NamedTunnelConfig {
            tunnel_name: "prod".into(),
            credentials_file: Some("/tmp/c.json".into()),
            hostname: Some("oxi.example.com".into()),
        };
        let text = toml::to_string(&cfg).unwrap();
        let parsed: NamedTunnelConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.tunnel_name, "prod");
        assert_eq!(parsed.credentials_file.as_deref(), Some("/tmp/c.json"));
    }

    #[test]
    fn hostname_optional() {
        let text = "tunnel_name = \"prod\"\n";
        let cfg: NamedTunnelConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.tunnel_name, "prod");
        assert!(cfg.hostname.is_none());
    }
}
