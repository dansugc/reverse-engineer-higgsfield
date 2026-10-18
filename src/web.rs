//! `reh web`: local upload UI. Binds 127.0.0.1 and rejects foreign Host headers.

use crate::{analyze, build, fal, flow::Flow, probe};
use anyhow::Result;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path as AxPath, Request},
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

const PAGE: &str = include_str!("../web/index.html");
const RUNS: &str = "reh-runs";

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

async fn status() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "harness": analyze::harness_sha(),
        "fal_key": fal::key_from_env().is_some(),
        "ffmpeg": probe::check_tools().is_ok(),
        "font": build::find_font().is_some(),
    }))
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

async fn analyze_upload(mut mp: Multipart) -> Response {
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
    let key = if no_ai { None } else { fal::Fal::from_env().ok() };
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

async fn plan(Json(flow): Json<Flow>) -> Response {
    if let Err(e) = flow.validate() {
        return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
    }
    Json(build::estimate(&flow, fal::Fal::from_env().ok().as_ref()).await).into_response()
}

async fn run_build(Json(req): Json<BuildReq>) -> Response {
    let Some(dir) = run_path(&req.run) else { return err(StatusCode::BAD_REQUEST, "bad run") };
    let fal = match fal::Fal::from_env() {
        Ok(f) => f,
        Err(e) => return err(StatusCode::PRECONDITION_FAILED, e),
    };
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
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/api/status", get(status))
        .route("/api/analyze", post(analyze_upload))
        .route("/api/plan", post(plan))
        .route("/api/build", post(run_build))
        .route("/files/{run}/{*rest}", get(files))
        .layer(DefaultBodyLimit::max(300 * 1024 * 1024))
        .layer(middleware::from_fn(host_guard));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let url = format!("http://localhost:{port}");
    println!("REH is running at {url}  (Ctrl+C to stop)");
    if open {
        let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(&url).spawn();
    }
    axum::serve(listener, app).await?;
    Ok(())
}
