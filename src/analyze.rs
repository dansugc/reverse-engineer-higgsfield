//! investigate → spec: probe the file, ask a video model to read it, and
//! normalize the answer into a valid Flow.

use crate::fal::{self, Fal};
use crate::flow::{self, Audio, Caption, Claim, Clip, Edit, Flow, Format, Kind, Step, Variable};
use crate::probe::{self, Probe};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const ANALYST: &str = include_str!("../harness/analyst.md");
pub const FORMATS: &str = include_str!("../harness/formats.md");
pub const MODELS: &str = include_str!("../harness/models.md");
pub const VERIFY: &str = include_str!("../harness/verify.md");
pub const DEFAULT_VIDEO_LLM: &str = "google/gemini-3.1-pro-preview";

pub fn harness_sha() -> String {
    let mut h = Sha256::new();
    for part in [ANALYST, FORMATS, MODELS, VERIFY] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

pub fn file_sha(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes).iter().take(8).map(|b| format!("{b:02x}")).collect())
}

pub struct Analysis {
    pub probe: Probe,
    pub flow: Flow,
    /// Raw model answer (shots, higgsfield_guess…) when AI ran.
    pub reading: Option<Value>,
}

pub fn system_prompt() -> String {
    format!("{ANALYST}\n\n---\n\n{FORMATS}\n\n---\n\n{MODELS}")
}

fn probe_brief(p: &Probe) -> Value {
    json!({
        "duration_s": p.duration_s, "width": p.width, "height": p.height, "aspect": p.aspect, "fps": p.fps,
        "has_audio": p.has_audio, "audio_mean_db": p.audio_mean_db, "provenance": p.provenance,
        "shots": p.shots.iter().map(|s| json!({ "index": s.index, "start_s": s.start_s, "end_s": s.end_s, "duration_s": s.duration_s })).collect::<Vec<_>>()
    })
}

/// Pull the first top-level JSON object out of a model answer (tolerates fences/prose).
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    serde_json::from_str(&t[start..=end]).ok()
}

pub async fn analyze(video: &Path, out_dir: &Path, fal: Option<&Fal>, model: &str, log: &mut (dyn FnMut(&str) + Send)) -> Result<Analysis> {
    log("probing file (ffprobe, scene cuts, keyframes)…");
    let probe = probe::probe(video, &out_dir.join("frames"))?;
    log(&format!("  {:.1}s · {}x{} ({}) · {} shot(s) · audio: {}", probe.duration_s, probe.width, probe.height, probe.aspect, probe.shots.len(), probe.has_audio));
    let sha = file_sha(video)?;

    let (reading, flow) = match fal {
        Some(fal) => {
            log(&format!("reading the video with {model} via fal (openrouter/router/video)…"));
            let prompt = format!(
                "Measured facts from the file (ground truth):\n{}\n\nReverse engineer this video into the JSON described in your instructions. Use exactly these {} shot(s).",
                serde_json::to_string_pretty(&probe_brief(&probe))?,
                probe.shots.len()
            );
            let input = fal::video_llm_input(model, &system_prompt(), &prompt, &[fal::data_uri(video)?]);
            let out = fal.run("openrouter/router/video", &input, Duration::from_secs(600), |s| log(&format!("  fal: {s}"))).await?;
            let text = fal::text_output(&out);
            std::fs::write(out_dir.join("reading.raw.txt"), &text)?;
            let reading = extract_json(&text).context("the video model did not return JSON (see reading.raw.txt)")?;
            let flow = normalize(&reading, &probe, &sha, model)?;
            (Some(reading), flow)
        }
        None => {
            log("no FAL_KEY — building a probe-only skeleton flow (prompts are placeholders; run again with a key for the real reading)");
            (None, skeleton(&probe, &sha))
        }
    };
    flow.validate()?;
    Ok(Analysis { probe, flow, reading })
}

fn source_info(p: &Probe, sha: &str, reader: &str) -> Value {
    json!({ "file": Path::new(&p.file).file_name().map(|f| f.to_string_lossy().into_owned()), "sha256_16": sha, "read_with": reader, "harness_sha": harness_sha(), "tool": concat!("reh ", env!("CARGO_PKG_VERSION")) })
}

fn format_of(p: &Probe) -> Format {
    Format { aspect: p.aspect.clone(), width: p.width, height: p.height, duration_s: p.duration_s, fps: p.fps }
}

fn observed(p: &Probe) -> Vec<Claim> {
    let mut c = vec![
        Claim { claim: format!("{:.2}s, {}x{} ({}), {:.0} fps", p.duration_s, p.width, p.height, p.aspect, p.fps), kind: "observed".into(), source: "ffprobe".into() },
        Claim {
            claim: format!("{} shot(s): {}", p.shots.len(), p.shots.iter().map(|s| format!("{:.2}–{:.2}s", s.start_s, s.end_s)).collect::<Vec<_>>().join(", ")),
            kind: "observed".into(),
            source: "ffmpeg scene detection (threshold 0.3)".into(),
        },
        Claim { claim: format!("audio track: {}", if p.has_audio { format!("yes (mean {:.1} dB)", p.audio_mean_db.unwrap_or(f64::NAN)) } else { "none".into() }), kind: "observed".into(), source: "ffprobe / volumedetect".into() },
    ];
    for h in &p.provenance {
        c.push(Claim { claim: h.clone(), kind: "observed".into(), source: "container tags / byte scan".into() });
    }
    c
}

/// Model answer → valid Flow. Unknown models fall back to defaults; durations come from the probe.
pub fn normalize(r: &Value, p: &Probe, sha: &str, reader: &str) -> Result<Flow> {
    let s = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
    let id_ok = |x: &str| x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !x.is_empty();
    let mut steps: Vec<Step> = vec![];
    for st in r["steps"].as_array().cloned().unwrap_or_default() {
        let kind: Kind = match serde_json::from_value(st["kind"].clone()) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let mut id = s(&st["id"]).to_lowercase().replace(['-', ' '], "_");
        if !id_ok(&id) {
            id = format!("step{}", steps.len() + 1);
        }
        let mut model = s(&st["model"]);
        let known = flow::model_info(&model).map(|m| m.kind == kind).unwrap_or(false);
        if !known {
            model = match kind {
                Kind::TextToImage => flow::DEFAULT_T2I,
                Kind::ImageEdit => flow::DEFAULT_EDIT,
                Kind::ImageToVideo | Kind::TextToVideo => flow::DEFAULT_I2V,
            }
            .to_string();
        }
        let kind = if kind == Kind::TextToVideo { Kind::ImageToVideo } else { kind };
        steps.push(Step {
            id,
            kind,
            model,
            prompt: s(&st["prompt"]),
            image_from: st["image_from"].as_str().map(|x| x.to_lowercase().replace(['-', ' '], "_")),
            duration_s: st["duration_s"].as_f64(),
            params: Map::new(),
            why: s(&st["why"]),
        });
    }
    // A text-to-video guess becomes image-to-video, so it needs a first frame.
    let image_ids: Vec<String> = steps.iter().filter(|x| matches!(x.kind, Kind::TextToImage | Kind::ImageEdit)).map(|x| x.id.clone()).collect();
    for st in steps.iter_mut().filter(|x| x.kind == Kind::ImageToVideo) {
        if st.image_from.as_ref().map(|f| !image_ids.contains(f)).unwrap_or(true) {
            st.image_from = image_ids.first().cloned();
        }
    }
    if image_ids.is_empty() {
        return Ok(skeleton(p, sha));
    }

    // Sequence follows the measured shots; durations are the measured ones.
    let video_ids: Vec<String> = steps.iter().filter(|x| x.kind == Kind::ImageToVideo).map(|x| x.id.clone()).collect();
    let named: Vec<String> = r["shot_steps"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|y| y.to_lowercase())).collect()).unwrap_or_default();
    let mut sequence = vec![];
    for (i, shot) in p.shots.iter().enumerate() {
        let step = named.get(i).filter(|n| video_ids.contains(n)).cloned().or_else(|| video_ids.get(i).cloned()).or_else(|| video_ids.last().cloned());
        if let Some(step) = step {
            if let Some(st) = steps.iter_mut().find(|x| x.id == step) {
                st.duration_s = Some(st.duration_s.unwrap_or(0.0).max(shot.duration_s));
            }
            sequence.push(Clip { step, duration_s: shot.duration_s });
        }
    }

    let mut variables = BTreeMap::new();
    if let Some(obj) = r["variables"].as_object() {
        for (k, v) in obj {
            let key = k.to_lowercase().replace(['-', ' '], "_");
            if id_ok(&key) {
                variables.insert(key, Variable { value: v["value"].as_str().map(str::to_string).unwrap_or_else(|| s(v)), description: s(&v["description"]) });
            }
        }
    }
    let captions = r["captions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let text = c["text"].as_str()?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            let start = c["start_s"].as_f64().unwrap_or(0.0).clamp(0.0, p.duration_s);
            let end = c["end_s"].as_f64().unwrap_or(p.duration_s).clamp(start, p.duration_s);
            Some(Caption { text, start_s: start, end_s: end, position: c["position"].as_str().unwrap_or("center").into(), size: c["size"].as_str().unwrap_or("medium").into() })
        })
        .collect();

    let mut evidence = observed(p);
    for e in r["evidence"].as_array().cloned().unwrap_or_default() {
        if let Some(claim) = e["claim"].as_str() {
            evidence.push(Claim { claim: claim.into(), kind: "inferred".into(), source: e["source"].as_str().unwrap_or("video model").into() });
        }
    }
    if let Some(g) = r.get("higgsfield_guess") {
        evidence.push(Claim {
            claim: format!("Likely format: {} (tools: {}; confidence {})", s(&g["format"]), s(&g["likely_tools"]), s(&g["confidence"])),
            kind: "inferred".into(),
            source: s(&g["evidence"]),
        });
    }
    let mut unknowns: Vec<String> = r["unknowns"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    for must in ["Exact generation prompts and seeds (not stored in the file)", "Which model Higgsfield used for each shot (inferred from visuals only)"] {
        if !unknowns.iter().any(|u| u.to_lowercase().contains(&must.to_lowercase()[..12])) {
            unknowns.push(must.into());
        }
    }

    let name = {
        let n = s(&r["name"]).to_lowercase().replace([' ', '_'], "-");
        if n.is_empty() { "reverse-engineered-format".into() } else { n }
    };
    Ok(Flow {
        version: "1".into(),
        name,
        title: { let t = s(&r["title"]); if t.is_empty() { "Reverse-engineered format".into() } else { t } },
        format_summary: s(&r["format_summary"]),
        format: format_of(p),
        variables,
        steps,
        edit: Edit {
            sequence,
            captions,
            audio: Audio { mode: { let m = s(&r["audio"]["mode"]); if m.is_empty() { if p.has_audio { "music".into() } else { "none".into() } } else { m } }, note: s(&r["audio"]["note"]), music_path: None },
        },
        evidence,
        unknowns,
        source: Some(source_info(p, sha, reader)),
    })
}

/// No-AI fallback: one character image + one i2v step per measured shot.
pub fn skeleton(p: &Probe, sha: &str) -> Flow {
    let mut steps = vec![Step {
        id: "character".into(),
        kind: Kind::TextToImage,
        model: flow::DEFAULT_T2I.into(),
        prompt: "{{subject}} in {{setting}}, {{style}}".into(),
        image_from: None,
        duration_s: None,
        params: Map::new(),
        why: "first frame for every shot (placeholder — describe the subject from the keyframes)".into(),
    }];
    let mut sequence = vec![];
    for s in &p.shots {
        let id = format!("shot{}", s.index);
        steps.push(Step {
            id: id.clone(),
            kind: Kind::ImageToVideo,
            model: flow::DEFAULT_I2V.into(),
            prompt: format!("{{{{shot{}_motion}}}}", s.index),
            image_from: Some("character".into()),
            duration_s: Some(s.duration_s),
            params: Map::new(),
            why: format!("shot {} ({:.2}–{:.2}s)", s.index, s.start_s, s.end_s),
        });
        sequence.push(Clip { step: id, duration_s: s.duration_s });
    }
    let mut variables = BTreeMap::new();
    for (k, v, d) in [("subject", "a person", "who/what is on screen"), ("setting", "a bright room", "where it happens"), ("style", "vertical iPhone UGC footage, natural light", "look and feel")] {
        variables.insert(k.into(), Variable { value: v.into(), description: d.into() });
    }
    for s in &p.shots {
        variables.insert(format!("shot{}_motion", s.index), Variable { value: "subtle natural motion, static camera".into(), description: format!("subject + camera motion in shot {}", s.index) });
    }
    Flow {
        version: "1".into(),
        name: "skeleton".into(),
        title: "Probe-only skeleton".into(),
        format_summary: format!("{} shot(s), {:.1}s, {} — fill the variables from the keyframes or re-run with FAL_KEY", p.shots.len(), p.duration_s, p.aspect),
        format: format_of(p),
        variables,
        steps,
        edit: Edit { sequence, captions: vec![], audio: Audio { mode: if p.has_audio { "music".into() } else { "none".into() }, note: String::new(), music_path: None } },
        evidence: observed(p),
        unknowns: vec!["Everything visual: no video model ran (set FAL_KEY)".into()],
        source: Some(source_info(p, sha, "probe only")),
    }
}

pub fn report_md(a: &Analysis, flow_path: &str) -> String {
    let f = &a.flow;
    let mut md = format!("# {}\n\n**Format:** {}\n\n", f.title, f.format_summary);
    md.push_str(&format!("`{:.1}s` · `{}x{}` · `{}` · {} shot(s) · harness `{}`\n\n", f.format.duration_s, f.format.width, f.format.height, f.format.aspect, a.probe.shots.len(), harness_sha()));
    if let Some(r) = &a.reading {
        if let Some(shots) = r["shots"].as_array() {
            md.push_str("## Shots\n\n| # | Time | What happens | Camera | Text |\n|---|---|---|---|---|\n");
            for (i, sh) in shots.iter().enumerate() {
                let t = a.probe.shots.get(i).map(|x| format!("{:.2}–{:.2}s", x.start_s, x.end_s)).unwrap_or_default();
                let cell = |k: &str| sh[k].as_str().unwrap_or("").replace('|', "/").replace('\n', " ");
                md.push_str(&format!("| {} | {} | {} | {} | {} |\n", i + 1, t, cell("description"), cell("camera"), cell("on_screen_text")));
            }
            md.push('\n');
        }
    }
    md.push_str("## The fal flow\n\n");
    for s in &f.steps {
        md.push_str(&format!("- **{}** · `{:?}` · `{}`{}{}\n  > {}\n", s.id, s.kind, s.model, s.image_from.as_ref().map(|x| format!(" ← {x}")).unwrap_or_default(), s.duration_s.map(|d| format!(" · {d:.1}s")).unwrap_or_default(), s.prompt.replace('\n', " ")));
    }
    md.push_str("\n**Edit:** ");
    md.push_str(&f.edit.sequence.iter().map(|c| format!("{} ({:.2}s)", c.step, c.duration_s)).collect::<Vec<_>>().join(" → "));
    for c in &f.edit.captions {
        md.push_str(&format!("\n- caption \"{}\" {:.1}–{:.1}s ({})", c.text, c.start_s, c.end_s, c.position));
    }
    md.push_str(&format!("\n- audio: {} {}\n\n## Variables (scale the format)\n\n", f.edit.audio.mode, f.edit.audio.note));
    for (k, v) in &f.variables {
        md.push_str(&format!("- `{{{{{k}}}}}` = \"{}\" — {}\n", v.value, v.description));
    }
    md.push_str("\n## Evidence\n\n");
    for e in &f.evidence {
        md.push_str(&format!("- _{}_: {} ({})\n", e.kind, e.claim, e.source));
    }
    md.push_str("\n## Unknowns\n\n");
    for u in &f.unknowns {
        md.push_str(&format!("- {u}\n"));
    }
    let total: f64 = f.rough_cost().iter().map(|c| c.2).sum();
    md.push_str(&format!(
        "\n## Next\n\n```bash\nreh plan {flow_path}                 # cost estimate (~${total:.2} per run, rough)\nreh build {flow_path} --max-usd 5    # rebuild on fal\nreh batch {flow_path} --vars variants.jsonl --max-usd 20\nreh publish {flow_path}              # create it as a fal Workflow\n```\n"
    ));
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Shot;

    fn probe2() -> Probe {
        Probe {
            file: "x.mp4".into(), bytes: 1, duration_s: 13.0, width: 720, height: 1280, fps: 30.0, aspect: "9:16".into(), has_audio: true, audio_mean_db: Some(-16.0),
            container_tags: Default::default(), provenance: vec!["TikTok re-encode".into()],
            shots: vec![Shot { index: 1, start_s: 0.0, end_s: 7.07, duration_s: 7.07, keyframes: vec![] }, Shot { index: 2, start_s: 7.07, end_s: 13.0, duration_s: 5.93, keyframes: vec![] }],
        }
    }

    #[test]
    fn extracts_json_from_fenced_answers() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```").unwrap()["a"], 1);
        assert!(extract_json("no json here").is_none());
    }

    #[test]
    fn normalizes_a_model_answer_into_a_valid_flow() {
        let r = serde_json::json!({
            "name": "Before After", "title": "Before/after", "format_summary": "2-shot transformation",
            "variables": { "character": { "value": "a woman in her 20s", "description": "who" }, "bad key!": { "value": "x" } },
            "steps": [
                { "id": "character", "kind": "text_to_image", "model": "some/unknown-model", "prompt": "{{character}}" },
                { "id": "after-frame", "kind": "image_edit", "model": "fal-ai/nano-banana-pro/edit", "prompt": "fitter", "image_from": "character" },
                { "id": "shot1", "kind": "image_to_video", "model": "fal-ai/kling-video/v3/pro/image-to-video", "prompt": "turns", "image_from": "character", "duration_s": 6 },
                { "id": "shot2", "kind": "text_to_video", "model": "x", "prompt": "turns, arms out", "image_from": "after-frame" }
            ],
            "shot_steps": ["shot1", "shot2"],
            "captions": [ { "text": "watch me lock in", "start_s": 0, "end_s": 99, "position": "center" } ],
            "audio": { "mode": "music", "note": "trending sound" },
            "evidence": [ { "claim": "same character across shots", "source": "faces" } ],
            "unknowns": []
        });
        let f = normalize(&r, &probe2(), "abc", "test").unwrap();
        f.validate().unwrap();
        assert_eq!(f.name, "before-after");
        assert_eq!(f.steps[0].model, crate::flow::DEFAULT_T2I); // unknown model replaced
        assert_eq!(f.steps[3].kind, Kind::ImageToVideo); // t2v → i2v
        assert_eq!(f.steps[3].image_from.as_deref(), Some("after_frame"));
        assert_eq!(f.edit.sequence.iter().map(|c| c.duration_s).collect::<Vec<_>>(), vec![7.07, 5.93]);
        assert_eq!(f.steps[2].duration_s, Some(7.07)); // raised to the measured shot
        assert_eq!(f.edit.captions[0].end_s, 13.0); // clamped
        assert!(!f.variables.contains_key("bad key!"));
        assert!(f.unknowns.len() >= 2);
    }

    #[test]
    fn skeleton_is_valid() {
        skeleton(&probe2(), "abc").validate().unwrap();
    }
}
