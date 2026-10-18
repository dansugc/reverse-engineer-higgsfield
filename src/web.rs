//! `reh web`: local upload UI. Binds 127.0.0.1 and rejects foreign Host headers.
//! The fal key can be pasted in the page: it is checked against fal, kept in
//! memory (optionally saved to ./.env, mode 600) and never sent back.

use crate::{analyze, build, fal, flow::Flow, probe};
use anyhow::Result;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path as AxPath, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

const PAGE: &str = include_str!("../web/index.html");
const RUNS: &str = "reh-runs";

#[derive(Clone, Default)]
struct AppState {
    key: Arc<RwLock<Option<String>>>,
}

impl AppState {
    fn fal(&self) -> Option<fal::Fal> {
        self.key.read().ok()?.clone().map(fal::Fal::new)
    }
}

fn err(status: StatusCode, msg: impl ToString) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

async fn host_guard(headers: HeaderMap, req: Request, next: Next) -> Response {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    if name != "localhost" && name != "127.0.0.1" {
        return err(StatusCode::FORBIDDEN, "forbidden host");
    }
    if req.method() != axum::http::Method::GET {
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
            if !origin.ends_with(host) {
                return err(StatusCode::FORBIDDEN, "forbidden origin");
            }
        }
    }
    next.run(req).await
}

async fn status(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "harness": analyze::harness_sha(),
        "fal_key": st.fal().is_some(),
        "ffmpeg": probe::check_tools().is_ok(),
        "font": build::find_font().is_some(),
    }))
}

#[derive(Deserialize)]
struct KeyReq {
    key: String,
    #[serde(default)]
    remember: bool,
}

/// Paste a fal key in the page. Empty key = forget it for this session.
async fn set_key(State(st): State<AppState>, Json(req): Json<KeyReq>) -> Response {
    let k = req.key.trim().to_string();
    if k.is_empty() {
        if let Ok(mut g) = st.key.write() {
            *g = None;
        }
        return Json(json!({ "fal_key": false })).into_response();
    }
    if !fal::plausible_key(&k) {
        return err(StatusCode::BAD_REQUEST, "That doesn't look like a fal key. It looks like `xxxxxxxx-xxxx-…:xxxxxxxx` (fal.ai → Dashboard → Keys).");
    }
    if let Err(e) = fal::Fal::new(k.clone()).check_key().await {
        return err(StatusCode::UNAUTHORIZED, format!("{e:#}. Copy the whole key, including the part after the colon."));
    }
    let mut saved = false;
    if req.remember {
        match save_env_key(&k) {
            Ok(()) => saved = true,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("key works, but saving .env failed: {e:#}")),
        }
    }
    if let Ok(mut g) = st.key.write() {
        *g = Some(k);
    }
    Json(json!({ "fal_key": true, "saved": saved })).into_response()
}

/// Write FAL_KEY into ./.env (gitignored), replacing any previous one; mode 600.
fn save_env_key(k: &str) -> Result<()> {
    let p = Path::new(".env");
    let mut lines: Vec<String> = std::fs::read_to_string(p).unwrap_or_default().lines().filter(|l| !l.starts_with("FAL_KEY=") && !l.starts_with("FAL_API_KEY=")).map(str::to_string).collect();
    lines.push(format!("FAL_KEY={k}"));
    std::fs::write(p, lines.join("\n") + "\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn run_path(run: &str) -> Option<PathBuf> {
    if run.is_empty() || !run.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(Path::new(RUNS).join(run))
}

fn file_url(run: &str, p: &str) -> String {
    let rel = Path::new(p).strip_prefix(Path::new(RUNS).join(run)).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string());
    format!("/files/{run}/{rel}")
}

async fn analyze_upload(State(st): State<AppState>, mut mp: Multipart) -> Response {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let run = format!("web-{ts}");
    let dir = Path::new(RUNS).join(&run);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let mut video: Option<PathBuf> = None;
    let mut no_ai = false;
    while let Ok(Some(field)) = mp.next_field().await {
        match field.name() {
            Some("file") => {
                let ext = field.file_name().and_then(|n| Path::new(n).extension().map(|e| e.to_string_lossy().to_lowercase())).filter(|e| ["mp4", "mov", "webm", "m4v"].contains(&e.as_str())).unwrap_or_else(|| "mp4".into());
                let bytes = match field.bytes().await {
                    Ok(b) => b,
                    Err(e) => return err(StatusCode::BAD_REQUEST, e),
                };
                let p = dir.join(format!("input.{ext}"));
                if let Err(e) = std::fs::write(&p, &bytes) {
                    return err(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
                video = Some(p);
            }
            Some("no_ai") => no_ai = field.text().await.map(|t| t == "true").unwrap_or(false),
            _ => {}
        }
    }
    let Some(video) = video else { return err(StatusCode::BAD_REQUEST, "no file") };
    let key = if no_ai { None } else { st.fal() };
    let mut logs: Vec<String> = vec![];
    let mut log = |m: &str| logs.push(m.to_string());
    let a = match analyze::analyze(&video, &dir, key.as_ref(), analyze::DEFAULT_VIDEO_LLM, &mut log).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")),
    };
    let flow_path = dir.join("flow.json");
    let _ = std::fs::write(&flow_path, serde_json::to_string_pretty(&a.flow).unwrap_or_default());
    let _ = std::fs::write(dir.join("workflow.fal.json"), serde_json::to_string_pretty(&a.flow.to_fal_workflow(&a.flow.name)).unwrap_or_default());
    let report = analyze::report_md(&a, &flow_path.to_string_lossy());
    let _ = std::fs::write(dir.join("REPORT.md"), &report);
    let est = build::estimate(&a.flow, key.as_ref()).await;
    let shots: Vec<Value> = a
        .probe
        .shots
        .iter()
        .map(|s| json!({ "index": s.index, "start_s": s.start_s, "end_s": s.end_s, "frames": s.keyframes.iter().map(|k| file_url(&run, k)).collect::<Vec<_>>() }))
        .collect();
    Json(json!({
        "run": run, "video": file_url(&run, &video.to_string_lossy()), "probe": a.probe, "shots": shots,
        "flow": a.flow, "reading": a.reading, "report": report, "estimate": est, "ai": key.is_some(), "logs": logs,
        "workflow": format!("/files/{run}/workflow.fal.json"),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct BuildReq {
    run: String,
    flow: Flow,
    #[serde(default)]
    set: BTreeMap<String, String>,
    max_usd: f64,
}

async fn plan(State(st): State<AppState>, Json(flow): Json<Flow>) -> Response {
    if let Err(e) = flow.validate() {
        return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
    }
    Json(build::estimate(&flow, st.fal().as_ref()).await).into_response()
}

async fn run_build(State(st): State<AppState>, Json(req): Json<BuildReq>) -> Response {
    let Some(dir) = run_path(&req.run) else { return err(StatusCode::BAD_REQUEST, "bad run") };
    let Some(fal) = st.fal() else { return err(StatusCode::PRECONDITION_FAILED, "Add your fal key first (top right).") };
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let out = dir.join(format!("build-{ts}"));
    let _ = std::fs::write(dir.join("flow.edited.json"), serde_json::to_string_pretty(&req.flow).unwrap_or_default());
    let log = |m: &str| eprintln!("{m}");
    match build::build(&req.flow, &fal, &out, &req.set, req.max_usd, None, &log).await {
        Ok(r) => Json(json!({
            "video": r.video.as_ref().map(|v| file_url(&req.run, v)),
            "steps": r.steps.iter().map(|s| json!({ "step": s.step, "model": s.model, "file": file_url(&req.run, &s.file), "url": s.url })).collect::<Vec<_>>(),
            "estimate": r.estimate, "warnings": r.warnings,
        }))
        .into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct PublishReq {
    #[serde(default)]
    run: String,
    flow: Flow,
    #[serde(default)]
    name: String,
}

/// Create the (edited) flow as a fal Workflow with the prompts inside its nodes.
async fn publish(State(st): State<AppState>, Json(req): Json<PublishReq>) -> Response {
    if let Err(e) = req.flow.validate() {
        return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
    }
    let Some(fal) = st.fal() else { return err(StatusCode::PRECONDITION_FAILED, "Add your fal key first (top right).") };
    let base = slug(if req.name.trim().is_empty() { &req.flow.name } else { &req.name });
    if let Some(dir) = run_path(&req.run) {
        let _ = std::fs::write(dir.join("flow.edited.json"), serde_json::to_string_pretty(&req.flow).unwrap_or_default());
        let _ = std::fs::write(dir.join("workflow.fal.json"), serde_json::to_string_pretty(&req.flow.to_fal_workflow(&base)).unwrap_or_default());
    }
    let vals = req.flow.values(&BTreeMap::new());
    let prompts: Vec<Value> = req.flow.steps.iter().map(|s| json!({ "step": s.id, "model": s.model, "prompt": crate::flow::render(&s.prompt, &vals) })).collect();
    match fal.create_workflow_unique(&base, |n| req.flow.to_fal_workflow(n)).await {
        Ok((name, url)) => Json(json!({ "name": name, "url": url, "prompts": prompts })).into_response(),
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}\nYou can still download “fal workflow.json” and import it in the fal Workflow editor.")),
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(60).collect();
    if out.is_empty() { "reh-flow".into() } else { out }
}

async fn files(AxPath((run, rest)): AxPath<(String, String)>) -> Response {
    let Some(base) = run_path(&run) else { return err(StatusCode::BAD_REQUEST, "bad run") };
    let rel = Path::new(&rest);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return err(StatusCode::BAD_REQUEST, "bad path");
    }
    let p = base.join(rel);
    let Ok(bytes) = tokio::fs::read(&p).await else { return err(StatusCode::NOT_FOUND, "not found") };
    let ct = match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "json" => "application/json",
        "md" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-store")], Body::from(bytes)).into_response()
}

pub async fn serve(port: u16, open: bool) -> Result<()> {
    std::fs::create_dir_all(RUNS)?;
    let state = AppState { key: Arc::new(RwLock::new(fal::key_from_env())) };
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/api/status", get(status))
        .route("/api/analyze", post(analyze_upload))
        .route("/api/plan", post(plan))
        .route("/api/build", post(run_build))
        .route("/api/key", post(set_key))
        .route("/api/publish", post(publish))
        .route("/files/{run}/{*rest}", get(files))
        .layer(DefaultBodyLimit::max(300 * 1024 * 1024))
        .layer(middleware::from_fn(host_guard))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let url = format!("http://localhost:{port}");
    println!("REH is running at {url}  (Ctrl+C to stop)");
    if open {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(&url).spawn();
    }
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_names_are_fal_safe() {
        assert_eq!(slug("Fitness Before/After  Transformation!"), "fitness-before-after-transformation");
        assert_eq!(slug("  "), "reh-flow");
        assert_eq!(slug(&"x".repeat(100)).len(), 60);
    }

    #[test]
    fn junk_keys_are_rejected_before_any_request() {
        assert!(!fal::plausible_key("hello"));
        assert!(!fal::plausible_key("sk-ant-api03-not-a-fal-key-at-all-xxxx"));
        assert!(!fal::plausible_key("abc:def ghi jkl mno pqr stu"));
        assert!(fal::plausible_key("00000000-0000-0000-0000-000000000000:0123456789abcdef0123456789abcdef"));
    }
}
