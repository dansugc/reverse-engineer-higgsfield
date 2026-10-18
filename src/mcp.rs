//! Minimal MCP server over stdio (JSON-RPC 2.0, one message per line).
//! Tools mirror the CLI; prompts are REA-style guided workflows.

use crate::{analyze, build, fal, flow, load_flow, run_dir, verify};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn tools() -> Value {
    json!([
        { "name": "analyze_video", "description": "Reverse engineer a local AI video (Higgsfield, Kling, Seedance…): measured shots/timing/keyframes + a video-model reading → flow.json (fal.ai recipe), REPORT.md and a fal Workflow JSON. Returns the report and the flow path.",
          "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "absolute path to the video" }, "no_ai": { "type": "boolean" }, "model": { "type": "string" } }, "required": ["path"] } },
        { "name": "plan_flow", "description": "Steps and estimated fal cost of one run of a flow (no spending).",
          "inputSchema": { "type": "object", "properties": { "flow_path": { "type": "string" } }, "required": ["flow_path"] } },
        { "name": "get_flow", "description": "Read a flow.json (to edit prompts/variables before building).",
          "inputSchema": { "type": "object", "properties": { "flow_path": { "type": "string" } }, "required": ["flow_path"] } },
        { "name": "build_flow", "description": "SPENDS fal credits: rebuild the video on fal from a flow, then assemble it (trim to original cuts, captions). Requires confirm=true and a max_usd budget; refuses if the estimate is higher.",
          "inputSchema": { "type": "object", "properties": { "flow_path": { "type": "string" }, "max_usd": { "type": "number" }, "set": { "type": "object", "additionalProperties": { "type": "string" }, "description": "variable overrides" }, "music_path": { "type": "string" }, "confirm": { "type": "boolean" } }, "required": ["flow_path", "max_usd", "confirm"] } },
        { "name": "verify_rebuild", "description": "Compare a rebuild with the original: structural checks + video-model judge scores and prompt fixes. apply=true writes flow.v2.json.",
          "inputSchema": { "type": "object", "properties": { "original": { "type": "string" }, "rebuild": { "type": "string" }, "flow_path": { "type": "string" }, "apply": { "type": "boolean" } }, "required": ["original", "rebuild"] } },
        { "name": "publish_workflow", "description": "Export the flow as a native fal Workflow JSON and (unless dry_run) create it on fal (key needs workflows:write).",
          "inputSchema": { "type": "object", "properties": { "flow_path": { "type": "string" }, "name": { "type": "string" }, "dry_run": { "type": "boolean" } }, "required": ["flow_path"] } },
        { "name": "get_playbook", "description": "REH's analyst instructions: evidence rules, Higgsfield format catalog, fal model guide, verify rubric.",
          "inputSchema": { "type": "object", "properties": {} } }
    ])
}

fn prompts() -> Value {
    json!([
        { "name": "investigate_video", "title": "Investigate a video", "description": "Reverse engineer an AI video into a fal flow, with evidence and unknowns.", "arguments": [ { "name": "path", "required": true } ] },
        { "name": "rebuild_on_fal", "title": "Rebuild on fal (spec → build → verify)", "description": "Plan, rebuild within a budget, verify against the original, apply fixes.", "arguments": [ { "name": "path", "required": true }, { "name": "max_usd", "required": false } ] },
        { "name": "scale_format", "title": "Scale the format", "description": "Turn a working flow into N variants (new characters/products/captions) within a budget.", "arguments": [ { "name": "flow_path", "required": true }, { "name": "variants", "required": false } ] }
    ])
}

fn prompt_text(name: &str, a: &Value) -> Result<String> {
    let arg = |k: &str| a[k].as_str().unwrap_or("").to_string();
    Ok(match name {
        "investigate_video" => format!("Call analyze_video({{path: \"{}\"}}). Explain the format in one line, walk through the shot table, the fal flow (each step's model and why), the variables that make it scalable, and the evidence vs unknowns. Do not claim the exact Higgsfield prompt, seed, or model — they are not in the file.", arg("path")),
        "rebuild_on_fal" => format!("Investigate → spec → build → verify. 1) analyze_video({{path: \"{p}\"}}). 2) plan_flow and show me the cost. 3) After I agree, build_flow with max_usd {b} and confirm=true. 4) verify_rebuild(original: \"{p}\", rebuild: <result>, flow_path, apply: true). 5) If structure or scores are weak, show the fixes and ask before one more build with flow.v2.json.", p = arg("path"), b = if arg("max_usd").is_empty() { "5".into() } else { arg("max_usd") }),
        "scale_format" => format!("Read get_flow({{flow_path: \"{}\"}}). Propose {} variants as variable overrides (new characters, products, settings, captions) that keep the format intact. Show the total cost from plan_flow × variants, then wait for my go before calling build_flow for each with confirm=true. Never use a real person's identity.", arg("flow_path"), if arg("variants").is_empty() { "5".into() } else { arg("variants") }),
        _ => bail!("unknown prompt {name}"),
    })
}

async fn call(name: &str, a: &Value) -> Result<String> {
    let s = |k: &str| a[k].as_str().map(str::to_string);
    match name {
        "analyze_video" => {
            let path = PathBuf::from(s("path").ok_or_else(|| anyhow!("path required"))?);
            let dir = run_dir(None, &path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into()));
            std::fs::create_dir_all(&dir)?;
            let key = if a["no_ai"].as_bool().unwrap_or(false) { None } else { fal::Fal::from_env().ok() };
            let model = s("model").unwrap_or_else(|| analyze::DEFAULT_VIDEO_LLM.into());
            let mut log = |m: &str| eprintln!("{m}");
            let an = analyze::analyze(&path, &dir, key.as_ref(), &model, &mut log).await?;
            let flow_path = dir.join("flow.json");
            std::fs::write(&flow_path, serde_json::to_string_pretty(&an.flow)?)?;
            std::fs::write(dir.join("workflow.fal.json"), serde_json::to_string_pretty(&an.flow.to_fal_workflow(&an.flow.name))?)?;
            let report = analyze::report_md(&an, &flow_path.to_string_lossy());
            std::fs::write(dir.join("REPORT.md"), &report)?;
            Ok(format!("flow_path: {}\nkeyframes: {}\n\n{report}", flow_path.display(), dir.join("frames").display()))
        }
        "plan_flow" => {
            let f = load_flow(&PathBuf::from(s("flow_path").unwrap_or_default()))?;
            Ok(serde_json::to_string_pretty(&build::estimate(&f, fal::Fal::from_env().ok().as_ref()).await)?)
        }
        "get_flow" => Ok(std::fs::read_to_string(s("flow_path").unwrap_or_default())?),
        "build_flow" => {
            if !a["confirm"].as_bool().unwrap_or(false) {
                bail!("build_flow spends fal credits: show the plan to the user and call again with confirm=true");
            }
            let path = PathBuf::from(s("flow_path").unwrap_or_default());
            let f = load_flow(&path)?;
            let max = a["max_usd"].as_f64().ok_or_else(|| anyhow!("max_usd required"))?;
            let overrides: BTreeMap<String, String> = a["set"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            let fal = fal::Fal::from_env()?;
            let dir = run_dir(None, &format!("{}-build", f.name));
            let log = |m: &str| eprintln!("{m}");
            let r = build::build(&f, &fal, &dir, &overrides, max, s("music_path").as_deref().map(std::path::Path::new), &log).await?;
            Ok(serde_json::to_string_pretty(&build::summary_json(&r))?)
        }
        "verify_rebuild" => {
            let (o, r) = (PathBuf::from(s("original").unwrap_or_default()), PathBuf::from(s("rebuild").unwrap_or_default()));
            let work = run_dir(None, "verify");
            std::fs::create_dir_all(&work)?;
            let structural = verify::structural(&o, &r, &work)?;
            let flow_path = s("flow_path").map(PathBuf::from);
            let flow = flow_path.as_ref().map(|p| load_flow(p)).transpose()?;
            let judge = match (fal::Fal::from_env().ok(), &flow) {
                (Some(fal), Some(f)) => Some(verify::judge(&o, &r, f, &fal, analyze::DEFAULT_VIDEO_LLM).await?),
                _ => None,
            };
            let v = verify::Verdict { structural, judge };
            let mut text = verify::render(&v);
            if let (true, Some(j), Some(f), Some(p)) = (a["apply"].as_bool().unwrap_or(false), &v.judge, &flow, &flow_path) {
                let (f2, applied) = verify::apply_fixes(f, j);
                let out = p.with_file_name("flow.v2.json");
                std::fs::write(&out, serde_json::to_string_pretty(&f2)?)?;
                text.push_str(&format!("\n\nApplied {} fix(es) → {}", applied.len(), out.display()));
            }
            Ok(text)
        }
        "publish_workflow" => {
            let path = PathBuf::from(s("flow_path").unwrap_or_default());
            let f: flow::Flow = load_flow(&path)?;
            let name = s("name").unwrap_or_else(|| format!("reh-{}", f.name));
            let wf = f.to_fal_workflow(&name);
            let out = path.with_file_name("workflow.fal.json");
            std::fs::write(&out, serde_json::to_string_pretty(&wf)?)?;
            if a["dry_run"].as_bool().unwrap_or(false) {
                return Ok(format!("wrote {}", out.display()));
            }
            match fal::Fal::from_env()?.create_workflow(&wf).await {
                Ok(v) => Ok(format!("created fal Workflow `{name}`; wrote {}\n{}", out.display(), v)),
                Err(e) => Ok(format!("wrote {} (import it in the fal Workflow editor). Create failed: {e:#}", out.display())),
            }
        }
        "get_playbook" => Ok(format!("{}\n\n{}\n\n{}\n\n{}", analyze::ANALYST, analyze::FORMATS, analyze::MODELS, analyze::VERIFY)),
        _ => bail!("unknown tool {name}"),
    }
}

pub async fn serve() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(id) = msg.get("id").cloned() else { continue }; // notification
        let method = msg["method"].as_str().unwrap_or("");
        let params = &msg["params"];
        let result: Result<Value> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": {}, "prompts": {} },
                "serverInfo": { "name": "reverse-engineer-higgsfield", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Reverse engineer AI videos into fal.ai flows. Start with analyze_video. build_flow spends money: always show plan_flow first and get the user's go."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "prompts/list" => Ok(json!({ "prompts": prompts() })),
            "prompts/get" => prompt_text(params["name"].as_str().unwrap_or(""), &params["arguments"]).map(|t| json!({ "messages": [ { "role": "user", "content": { "type": "text", "text": t } } ] })),
            "tools/call" => Ok(match call(params["name"].as_str().unwrap_or(""), &params["arguments"]).await {
                Ok(text) => json!({ "content": [ { "type": "text", "text": text } ] }),
                Err(e) => json!({ "content": [ { "type": "text", "text": format!("error: {e:#}") } ], "isError": true }),
            }),
            _ => Err(anyhow!("method not found: {method}")),
        };
        let reply = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{e:#}") } }),
        };
        stdout.write_all(format!("{reply}\n").as_bytes()).await?;
        stdout.flush().await?;
    }
    Ok(())
}
