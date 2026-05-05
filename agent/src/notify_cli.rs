// `oxiremote notify` subcommand. Reads the local notify token, POSTs to the
// running agent's /api/notify, prints status, exits. No clap dependency — the
// flag set is tiny and stable.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

const USAGE: &str = "Usage: oxiremote notify --title <text> [--body <text>] [--deep-link /h/<host>/...] [--urgency low|normal|high] [--ttl <seconds>] [--device-id <id>]";

#[derive(Default)]
struct Args {
    title: Option<String>,
    body: Option<String>,
    deep_link: Option<String>,
    urgency: Option<String>,
    ttl: Option<u32>,
    device_id: Option<String>,
}

fn parse_args(argv: &[String]) -> Result<Args> {
    let mut args = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let value = || -> Result<&str> {
            argv.get(i + 1).map(String::as_str).ok_or_else(|| anyhow!("{flag} needs a value"))
        };
        match flag {
            "--title" | "-t" => {
                args.title = Some(value()?.to_string());
                i += 2;
            }
            "--body" | "-b" => {
                args.body = Some(value()?.to_string());
                i += 2;
            }
            "--deep-link" | "-l" => {
                args.deep_link = Some(value()?.to_string());
                i += 2;
            }
            "--urgency" | "-u" => {
                args.urgency = Some(value()?.to_string());
                i += 2;
            }
            "--ttl" => {
                args.ttl = Some(value()?.parse().context("--ttl must be u32")?);
                i += 2;
            }
            "--device-id" | "-d" => {
                args.device_id = Some(value()?.to_string());
                i += 2;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown flag: {other}\n{USAGE}")),
        }
    }
    if args.title.is_none() {
        return Err(anyhow!("--title required\n{USAGE}"));
    }
    Ok(args)
}

fn default_data_dir() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME or USERPROFILE not set")?;
    Ok(Path::new(&home).join(".oxiremote"))
}

pub fn run(argv: Vec<String>) -> Result<()> {
    let args = parse_args(&argv)?;
    let data_dir = default_data_dir()?;
    let token_path = data_dir.join("notify.token");
    let token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("read {} — is the agent running?", token_path.display()))?
        .trim()
        .to_string();

    let mut payload = serde_json::Map::new();
    payload.insert("title".into(), serde_json::json!(args.title.unwrap()));
    if let Some(b) = args.body {
        payload.insert("body".into(), serde_json::json!(b));
    }
    if let Some(l) = args.deep_link {
        payload.insert("deep_link".into(), serde_json::json!(l));
    }
    if let Some(u) = args.urgency {
        payload.insert("urgency".into(), serde_json::json!(u));
    }
    if let Some(t) = args.ttl {
        payload.insert("ttl".into(), serde_json::json!(t));
    }
    if let Some(d) = args.device_id {
        payload.insert("device_id".into(), serde_json::json!(d));
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(send_notify(token, serde_json::Value::Object(payload)))
}

async fn send_notify(token: String, body: serde_json::Value) -> Result<()> {
    let client = reqwest::Client::new();
    let body_bytes = serde_json::to_vec(&body)?;
    let resp = client
        .post(format!("http://127.0.0.1:{}/api/notify", crate::AGENT_PORT))
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .context("POST /api/notify")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        println!("{text}");
        Ok(())
    } else {
        Err(anyhow!("notify failed: {status} — {text}"))
    }
}
