//! Minimal fal.ai client: queue API (submit → poll → result), file inputs,
//! pricing, and Workflows (Platform API).

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

const QUEUE: &str = "https://queue.fal.run";
const PLATFORM: &str = "https://api.fal.ai/v1";

#[derive(Clone)]
pub struct Fal {
    key: String,
    http: reqwest::Client,
}

pub fn key_from_env() -> Option<String> {
    std::env::var("FAL_KEY").ok().or_else(|| std::env::var("FAL_API_KEY").ok()).filter(|k| !k.trim().is_empty())
}

impl Fal {
    pub fn new(key: String) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(120)).user_agent(concat!("reh/", env!("CARGO_PKG_VERSION"))).build().expect("http client");
        Self { key, http }
    }

    pub fn from_env() -> Result<Self> {
        key_from_env().map(Self::new).ok_or_else(|| anyhow!("FAL_KEY is not set. Put it in .env (FAL_KEY=…) or export it."))
    }

    fn auth(&self) -> String {
        format!("Key {}", self.key)
    }

    /// Submit to the queue and wait for the result. `on_status` gets coarse progress.
    pub async fn run(&self, endpoint: &str, input: &Value, timeout: Duration, mut on_status: impl FnMut(&str)) -> Result<Value> {
        let res = self.http.post(format!("{QUEUE}/{endpoint}")).header("Authorization", self.auth()).json(input).send().await?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("fal {endpoint} rejected the request ({status}): {}", short(&body));
        }
        let status_url = body["status_url"].as_str().context("no status_url")?.to_string();
        let response_url = body["response_url"].as_str().context("no response_url")?.to_string();
        let started = Instant::now();
        let mut last = String::new();
        loop {
            if started.elapsed() > timeout {
                bail!("fal {endpoint} timed out after {:?}", timeout);
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let s: Value = self.http.get(&status_url).header("Authorization", self.auth()).send().await?.json().await?;
            let st = s["status"].as_str().unwrap_or("").to_string();
            if st != last {
                on_status(&st);
                last = st.clone();
            }
            match st.as_str() {
                "COMPLETED" => break,
                "IN_QUEUE" | "IN_PROGRESS" => continue,
                other if !other.is_empty() => bail!("fal {endpoint} ended with status {other}: {}", short(&s)),
                _ => continue,
            }
        }
        let res = self.http.get(&response_url).header("Authorization", self.auth()).send().await?;
        let status = res.status();
        let out: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() || out.get("detail").is_some() && out.get("video").is_none() && out.get("images").is_none() && out.get("output").is_none() {
            bail!("fal {endpoint} failed ({status}): {}", short(&out));
        }
        Ok(out)
    }

    /// Unit prices for endpoints: endpoint → (unit_price, unit, currency).
    pub async fn pricing(&self, endpoints: &[String]) -> Result<Vec<(String, f64, String, String)>> {
        if endpoints.is_empty() {
            return Ok(vec![]);
        }
        let url = format!("{PLATFORM}/models/pricing?endpoint_id={}", endpoints.join(","));
        let v: Value = self.http.get(url).header("Authorization", self.auth()).send().await?.json().await?;
        let prices = v["prices"].as_array().cloned().unwrap_or_default();
        Ok(prices
            .iter()
            .filter_map(|p| Some((p["endpoint_id"].as_str()?.to_string(), p["unit_price"].as_f64()?, p["unit"].as_str().unwrap_or("").to_string(), p["currency"].as_str().unwrap_or("USD").to_string())))
            .collect())
    }

    /// Create a fal Workflow (needs a key with `workflows:write`).
    pub async fn create_workflow(&self, body: &Value) -> Result<Value> {
        let res = self.http.post(format!("{PLATFORM}/workflows")).header("Authorization", self.auth()).json(body).send().await?;
        let status = res.status();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("fal Workflows API refused ({status}): {} — the key may lack `workflows:write` (SERVERLESS or FULL preset)", short(&v));
        }
        Ok(v)
    }

    /// Cheap authenticated call to check the key works (pricing of a known endpoint).
    pub async fn check_key(&self) -> Result<()> {
        let res = self.http.get(format!("{PLATFORM}/models/pricing?endpoint_id=fal-ai/nano-banana-pro")).header("Authorization", self.auth()).send().await?;
        if res.status().as_u16() == 401 || res.status().as_u16() == 403 {
            bail!("fal rejected the key ({})", res.status());
        }
        Ok(())
    }
}

/// Local file → data URI (fal inputs accept URLs or data URIs).
pub fn data_uri(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    if bytes.len() > 40 * 1024 * 1024 {
        bail!("{} is {} MB — too large to inline; trim or compress it first", path.display(), bytes.len() / 1_048_576);
    }
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// First media URL in a fal output (video.url, images[0].url, image.url, audio.url).
pub fn media_url(out: &Value) -> Option<String> {
    for p in ["/video/url", "/images/0/url", "/image/url", "/audio/url", "/output/url"] {
        if let Some(u) = out.pointer(p).and_then(|v| v.as_str()) {
            return Some(u.to_string());
        }
    }
    None
}

pub async fn download(url: &str, to: &Path) -> Result<()> {
    let bytes = reqwest::get(url).await?.error_for_status()?.bytes().await?;
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(to, &bytes)?;
    Ok(())
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 400 {
        format!("{}…", &s[..400])
    } else {
        s
    }
}

/// Text output of openrouter/router/video etc.
pub fn text_output(out: &Value) -> String {
    out["output"].as_str().map(str::to_string).unwrap_or_else(|| out.to_string())
}

pub fn video_llm_input(model: &str, system: &str, prompt: &str, video_urls: &[String]) -> Value {
    json!({ "model": model, "system_prompt": system, "prompt": prompt, "video_urls": video_urls, "temperature": 0.2, "max_tokens": 8000 })
}
