//! verify: does the rebuild reproduce the original's format? Structural checks
//! (measured) + a video-model judge (inferred), then concrete prompt fixes.

use crate::analyze::{self, VERIFY};
use crate::fal::{self, Fal};
use crate::flow::Flow;
use crate::probe;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub original: String,
    pub rebuild: String,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct Verdict {
    pub structural: Vec<Check>,
    pub judge: Option<Value>,
}

pub fn structural(original: &Path, rebuild: &Path, work: &Path) -> Result<Vec<Check>> {
    let a = probe::probe(original, &work.join("orig_frames"))?;
    let b = probe::probe(rebuild, &work.join("rebuild_frames"))?;
    let cuts = |p: &probe::Probe| p.shots.iter().skip(1).map(|s| s.start_s).collect::<Vec<_>>();
    let (ca, cb) = (cuts(&a), cuts(&b));
    let cut_ok = ca.len() == cb.len() && ca.iter().zip(&cb).all(|(x, y)| (x - y).abs() <= 0.35);
    Ok(vec![
        Check { name: "duration".into(), original: format!("{:.2}s", a.duration_s), rebuild: format!("{:.2}s", b.duration_s), ok: (a.duration_s - b.duration_s).abs() <= 0.5 },
        Check { name: "aspect".into(), original: a.aspect.clone(), rebuild: b.aspect.clone(), ok: a.aspect == b.aspect },
        Check { name: "shots".into(), original: a.shots.len().to_string(), rebuild: b.shots.len().to_string(), ok: a.shots.len() == b.shots.len() },
        Check {
            name: "cut timing (±0.35s)".into(),
            original: ca.iter().map(|c| format!("{c:.2}")).collect::<Vec<_>>().join(", "),
            rebuild: cb.iter().map(|c| format!("{c:.2}")).collect::<Vec<_>>().join(", "),
            ok: cut_ok,
        },
    ])
}

pub async fn judge(original: &Path, rebuild: &Path, flow: &Flow, fal: &Fal, model: &str) -> Result<Value> {
    let prompt = format!("ORIGINAL is the first video, REBUILD is the second. The flow that produced the rebuild:\n{}", serde_json::to_string_pretty(flow)?);
    let input = fal::video_llm_input(model, VERIFY, &prompt, &[fal::data_uri(original)?, fal::data_uri(rebuild)?]);
    let out = fal.run("openrouter/router/video", &input, Duration::from_secs(600), |_| {}).await?;
    analyze::extract_json(&fal::text_output(&out)).context("judge did not return JSON")
}

/// Apply the judge's prompt fixes to a copy of the flow.
pub fn apply_fixes(flow: &Flow, judge: &Value) -> (Flow, Vec<String>) {
    let mut f = flow.clone();
    let mut applied = vec![];
    for fix in judge["fixes"].as_array().cloned().unwrap_or_default() {
        let (Some(step), Some(change)) = (fix["step"].as_str(), fix["change"].as_str()) else { continue };
        if fix["field"].as_str().unwrap_or("prompt") != "prompt" || change.trim().len() < 8 {
            continue;
        }
        if let Some(s) = f.steps.iter_mut().find(|s| s.id == step) {
            s.prompt = change.trim().to_string();
            applied.push(format!("{step}: {}", fix["why"].as_str().unwrap_or("")));
        }
    }
    (f, applied)
}

pub fn render(v: &Verdict) -> String {
    let mut md = String::from("# Verify\n\n| Check | Original | Rebuild | |\n|---|---|---|---|\n");
    for c in &v.structural {
        md.push_str(&format!("| {} | {} | {} | {} |\n", c.name, c.original, c.rebuild, if c.ok { "✓" } else { "✗" }));
    }
    if let Some(j) = &v.judge {
        md.push_str("\n## Judge (video model — inferred)\n\n");
        if let Some(s) = j["scores"].as_object() {
            md.push_str(&s.iter().map(|(k, v)| format!("{k}: {v}/10")).collect::<Vec<_>>().join(" · "));
            md.push('\n');
        }
        for d in j["differences"].as_array().cloned().unwrap_or_default() {
            md.push_str(&format!("- {}\n", d.as_str().unwrap_or("")));
        }
        if let Some(fixes) = j["fixes"].as_array().filter(|f| !f.is_empty()) {
            md.push_str("\n### Suggested fixes\n\n");
            for f in fixes {
                md.push_str(&format!("- **{}**: {} — {}\n", f["step"].as_str().unwrap_or("?"), f["change"].as_str().unwrap_or(""), f["why"].as_str().unwrap_or("")));
            }
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_prompt_fixes_to_known_steps() {
        let flow: Flow = serde_json::from_value(serde_json::json!({
            "version": "1", "name": "t", "title": "T", "format_summary": "",
            "format": { "aspect": "9:16", "width": 720, "height": 1280, "duration_s": 5.0, "fps": 30.0 },
            "variables": {},
            "steps": [ { "id": "img", "kind": "text_to_image", "model": "fal-ai/nano-banana-pro", "prompt": "old" } ],
            "edit": { "sequence": [] }
        }))
        .unwrap();
        let judge = serde_json::json!({ "fixes": [
            { "step": "img", "field": "prompt", "change": "a much better prompt", "why": "closer framing" },
            { "step": "nope", "field": "prompt", "change": "ignored because unknown", "why": "" },
            { "step": "img", "field": "model", "change": "other/model", "why": "" }
        ] });
        let (f, applied) = apply_fixes(&flow, &judge);
        assert_eq!(f.steps[0].prompt, "a much better prompt");
        assert_eq!(applied.len(), 1);
    }
}
