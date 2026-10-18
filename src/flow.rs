//! The flow spec: a reproducible, editable recipe for a video format, built
//! from evidence. It runs locally via the fal queue (`reh build`) or exports as
//! a native fal Workflow (`reh publish`).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub version: String,
    pub name: String,
    pub title: String,
    /// What the format is, in one line ("before/after body transformation, 2 shots").
    pub format_summary: String,
    pub format: Format,
    /// `{{name}}` placeholders used in prompts and captions — the knobs for scaling the format.
    pub variables: BTreeMap<String, Variable>,
    pub steps: Vec<Step>,
    pub edit: Edit,
    #[serde(default)]
    pub evidence: Vec<Claim>,
    #[serde(default)]
    pub unknowns: Vec<String>,
    #[serde(default)]
    pub source: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Format {
    pub aspect: String,
    pub width: u32,
    pub height: u32,
    pub duration_s: f64,
    pub fps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub value: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    TextToImage,
    ImageEdit,
    ImageToVideo,
    TextToVideo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub kind: Kind,
    pub model: String,
    pub prompt: String,
    /// Step id whose image feeds this one (edit / image-to-video).
    #[serde(default)]
    pub image_from: Option<String>,
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// Raw model params merged over the adapter's defaults.
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Edit {
    pub sequence: Vec<Clip>,
    #[serde(default)]
    pub captions: Vec<Caption>,
    #[serde(default)]
    pub audio: Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub step: String,
    /// Seconds to keep from the start of the generated clip (trims to the original's cut).
    pub duration_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Caption {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
    #[serde(default = "center")]
    pub position: String,
    #[serde(default = "medium")]
    pub size: String,
}
fn center() -> String {
    "center".into()
}
fn medium() -> String {
    "medium".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Audio {
    /// none | music | keep_source
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub music_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub claim: String,
    /// observed (from the file) | inferred (model's reading of the pixels)
    pub kind: String,
    pub source: String,
}

// ---------------------------------------------------------------- models ---

pub struct ModelInfo {
    pub id: &'static str,
    pub kind: Kind,
    pub label: &'static str,
    /// Rough USD per output (per image, or per second for video) — used only when live pricing is unavailable.
    pub rough_usd: f64,
    pub per_second: bool,
    pub min_s: f64,
    pub max_s: f64,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo { id: "fal-ai/nano-banana-pro", kind: Kind::TextToImage, label: "Nano Banana Pro (t2i)", rough_usd: 0.15, per_second: false, min_s: 0.0, max_s: 0.0 },
    ModelInfo { id: "bytedance/seedream/v5/pro/text-to-image", kind: Kind::TextToImage, label: "Seedream 5 Pro (t2i)", rough_usd: 0.05, per_second: false, min_s: 0.0, max_s: 0.0 },
    ModelInfo { id: "fal-ai/nano-banana-pro/edit", kind: Kind::ImageEdit, label: "Nano Banana Pro edit (keeps identity)", rough_usd: 0.15, per_second: false, min_s: 0.0, max_s: 0.0 },
    ModelInfo { id: "bytedance/seedream/v5/pro/edit", kind: Kind::ImageEdit, label: "Seedream 5 Pro edit", rough_usd: 0.05, per_second: false, min_s: 0.0, max_s: 0.0 },
    ModelInfo { id: "fal-ai/kling-video/v3/pro/image-to-video", kind: Kind::ImageToVideo, label: "Kling v3 Pro (i2v)", rough_usd: 0.14, per_second: true, min_s: 3.0, max_s: 15.0 },
    ModelInfo { id: "fal-ai/kling-video/v3/standard/image-to-video", kind: Kind::ImageToVideo, label: "Kling v3 Standard (i2v)", rough_usd: 0.08, per_second: true, min_s: 3.0, max_s: 15.0 },
    ModelInfo { id: "bytedance/seedance-2.5/us/image-to-video", kind: Kind::ImageToVideo, label: "Seedance 2.5 (i2v)", rough_usd: 0.12, per_second: true, min_s: 4.0, max_s: 30.0 },
    ModelInfo { id: "fal-ai/minimax/hailuo-2.3/pro/image-to-video", kind: Kind::ImageToVideo, label: "Hailuo 2.3 Pro (i2v)", rough_usd: 0.08, per_second: true, min_s: 6.0, max_s: 6.0 },
];

pub const DEFAULT_T2I: &str = "fal-ai/nano-banana-pro";
pub const DEFAULT_EDIT: &str = "fal-ai/nano-banana-pro/edit";
pub const DEFAULT_I2V: &str = "fal-ai/kling-video/v3/pro/image-to-video";
pub const MERGE: &str = "fal-ai/ffmpeg-api/merge-videos";

pub fn model_info(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

fn clamp_secs(step: &Step, info: Option<&ModelInfo>) -> f64 {
    let want = step.duration_s.unwrap_or(5.0).ceil();
    match info {
        Some(i) if i.max_s > 0.0 => want.clamp(i.min_s, i.max_s),
        _ => want,
    }
}

const NANO_ASPECTS: &[&str] = &["21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16"];

/// Model-specific input JSON for one step. `image_url` may be a real URL or a
/// workflow reference like `$character.images.0.url`.
pub fn build_input(step: &Step, prompt: &str, image_url: Option<&str>, fmt: &Format) -> Value {
    let info = model_info(&step.model);
    let secs = clamp_secs(step, info);
    let aspect = if NANO_ASPECTS.contains(&fmt.aspect.as_str()) { fmt.aspect.clone() } else { "auto".into() };
    let mut v = match (step.model.as_str(), step.kind) {
        ("fal-ai/nano-banana-pro", _) => json!({ "prompt": prompt, "aspect_ratio": aspect, "resolution": "2K", "num_images": 1, "output_format": "png" }),
        ("fal-ai/nano-banana-pro/edit", _) => json!({ "prompt": prompt, "image_urls": [image_url], "aspect_ratio": aspect, "resolution": "2K", "num_images": 1, "output_format": "png" }),
        (m, _) if m.contains("seedream") && m.ends_with("/edit") => json!({ "prompt": prompt, "image_urls": [image_url], "num_images": 1 }),
        (m, _) if m.contains("seedream") => json!({ "prompt": format!("{prompt}\nVertical {} framing.", fmt.aspect), "num_images": 1 }),
        (m, _) if m.contains("kling-video") => json!({ "prompt": prompt, "start_image_url": image_url, "duration": format!("{secs}"), "generate_audio": false }),
        (m, _) if m.contains("seedance") => json!({ "prompt": prompt, "image_url": image_url, "duration": format!("{secs}"), "resolution": "720p", "generate_audio": false }),
        (m, _) if m.contains("hailuo") => json!({ "prompt": prompt, "image_url": image_url }),
        (_, Kind::TextToImage) => json!({ "prompt": prompt }),
        (_, Kind::ImageEdit) => json!({ "prompt": prompt, "image_urls": [image_url] }),
        (_, Kind::ImageToVideo) => json!({ "prompt": prompt, "image_url": image_url, "duration": format!("{secs}") }),
        (_, Kind::TextToVideo) => json!({ "prompt": prompt, "duration": format!("{secs}") }),
    };
    if let Some(obj) = v.as_object_mut() {
        for (k, val) in &step.params {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

/// Where a step's media lands in its output (for chaining and workflow refs).
pub fn output_path(kind: Kind) -> &'static str {
    match kind {
        Kind::TextToImage | Kind::ImageEdit => "images.0.url",
        Kind::ImageToVideo | Kind::TextToVideo => "video.url",
    }
}

fn label_of(id: &str) -> String {
    let s = id.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

// ------------------------------------------------------------- templating ---

pub fn render(template: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

impl Flow {
    pub fn values(&self, overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut m: BTreeMap<String, String> = self.variables.iter().map(|(k, v)| (k.clone(), v.value.clone())).collect();
        for (k, v) in overrides {
            m.insert(k.clone(), v.clone());
        }
        m
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen: BTreeMap<&str, Kind> = BTreeMap::new();
        for s in &self.steps {
            if s.id.is_empty() || !s.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                bail!("step id `{}` must be [A-Za-z0-9_]", s.id);
            }
            if seen.contains_key(s.id.as_str()) {
                bail!("duplicate step id `{}`", s.id);
            }
            if s.model.trim().is_empty() {
                bail!("step `{}` has no model", s.id);
            }
            let needs_image = matches!(s.kind, Kind::ImageEdit | Kind::ImageToVideo);
            match (&s.image_from, needs_image) {
                (None, true) => bail!("step `{}` ({:?}) needs image_from", s.id, s.kind),
                (Some(src), _) => match seen.get(src.as_str()) {
                    None => bail!("step `{}` image_from `{src}` is not an earlier step", s.id),
                    Some(k) if !matches!(k, Kind::TextToImage | Kind::ImageEdit) => bail!("step `{}` image_from `{src}` is not an image step", s.id),
                    _ => {}
                },
                _ => {}
            }
            seen.insert(&s.id, s.kind);
        }
        if self.edit.sequence.is_empty() {
            bail!("edit.sequence is empty — nothing to assemble");
        }
        for c in &self.edit.sequence {
            match seen.get(c.step.as_str()) {
                Some(Kind::ImageToVideo | Kind::TextToVideo) => {}
                _ => bail!("edit.sequence step `{}` is not a video step", c.step),
            }
        }
        for v in self.variables.keys() {
            if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                bail!("variable `{v}` must be [A-Za-z0-9_]");
            }
        }
        Ok(())
    }

    /// Rough cost (USD) of one full run, per step, from the built-in table.
    pub fn rough_cost(&self) -> Vec<(String, String, f64)> {
        self.steps
            .iter()
            .map(|s| {
                let info = model_info(&s.model);
                let usd = match info {
                    Some(i) if i.per_second => i.rough_usd * clamp_secs(s, Some(i)),
                    Some(i) => i.rough_usd,
                    None => match s.kind {
                        Kind::TextToImage | Kind::ImageEdit => 0.10,
                        _ => 0.12 * s.duration_s.unwrap_or(5.0),
                    },
                };
                (s.id.clone(), s.model.clone(), (usd * 100.0).round() / 100.0)
            })
            .collect()
    }

    /// Native fal Workflow definition: one `run` node per step, chained by
    /// `$node.path` refs, ending in an ffmpeg merge of the sequence.
    /// fal Workflow JSON in the shape the fal editor exports: `run` nodes with
    /// their prompts inline, chained by `$node.*` refs listed in `depends`, and
    /// one `display` node named `output`.
    pub fn to_fal_workflow(&self, name: &str) -> Value {
        let vals = self.values(&BTreeMap::new());
        let mut nodes = Map::new();
        let kinds: BTreeMap<&str, Kind> = self.steps.iter().map(|s| (s.id.as_str(), s.kind)).collect();
        for (i, s) in self.steps.iter().enumerate() {
            // The fal editor ignores schema defaults, so a `$input.*` prompt
            // shows up blank and fails as a missing required field. Prompts are
            // written into the nodes (editable there), like fal's own examples.
            let prompt = render(&s.prompt, &vals);
            let image_ref = s.image_from.as_ref().map(|src| format!("${src}.{}", output_path(kinds[src.as_str()])));
            let input = build_input(s, &prompt, image_ref.as_deref(), &self.format);
            let video = matches!(s.kind, Kind::ImageToVideo | Kind::TextToVideo);
            nodes.insert(
                s.id.clone(),
                json!({
                    "id": s.id, "type": "run", "app": s.model,
                    "depends": s.image_from.iter().cloned().collect::<Vec<_>>(),
                    "input": input,
                    "metadata": { "position": { "x": 520 * i as i64, "y": if video { 760 } else { 0 } } }
                }),
            );
        }
        let clips: Vec<String> = self.edit.sequence.iter().map(|c| format!("${}.video.url", c.step)).collect();
        let mut output = Map::new();
        let mut out_deps: Vec<String> = vec![];
        if clips.len() > 1 {
            let deps: Vec<String> = self.edit.sequence.iter().map(|c| c.step.clone()).collect();
            nodes.insert(
                "merge".into(),
                json!({ "id": "merge", "type": "run", "app": MERGE, "depends": deps, "input": { "video_urls": clips }, "metadata": { "position": { "x": 520 * self.steps.len() as i64, "y": 760 } } }),
            );
            output.insert("video".into(), json!("$merge.video.url"));
            out_deps.push("merge".into());
        } else if let Some(c) = clips.first() {
            output.insert("video".into(), json!(c));
        }
        for c in &self.edit.sequence {
            output.insert(format!("{}_clip", c.step), json!(format!("${}.video.url", c.step)));
            if !out_deps.contains(&c.step) {
                out_deps.push(c.step.clone());
            }
        }
        nodes.insert(
            "output".into(),
            json!({ "id": "output", "type": "display", "depends": out_deps, "fields": output, "input": {}, "metadata": { "position": { "x": 520 * (self.steps.len() + 1) as i64, "y": 300 } } }),
        );
        let out_schema: Map<String, Value> = output
            .keys()
            .map(|k| (k.clone(), json!({ "name": k, "label": if k == "video" { "Final video".to_string() } else { format!("{} clip", label_of(k.trim_end_matches("_clip"))) }, "type": "string" })))
            .collect();
        json!({
            "name": name,
            "title": self.title,
            "is_public": false,
            "contents": {
                "name": "workflow", "version": "1",
                "nodes": nodes, "output": output,
                "schema": { "input": {}, "output": out_schema },
                "metadata": {
                    "description": format!("{} — reverse-engineered by reh. Captions and trims to the original cuts are applied locally by `reh build`.", self.format_summary),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Flow {
        serde_json::from_value(json!({
            "version": "1", "name": "t", "title": "T", "format_summary": "two shots",
            "format": { "aspect": "9:16", "width": 720, "height": 1280, "duration_s": 13.0, "fps": 30.0 },
            "variables": { "character": { "value": "a woman in her 20s" } },
            "steps": [
                { "id": "character", "kind": "text_to_image", "model": "fal-ai/nano-banana-pro", "prompt": "{{character}} in a bathroom" },
                { "id": "after", "kind": "image_edit", "model": "fal-ai/nano-banana-pro/edit", "prompt": "same person, fitter", "image_from": "character" },
                { "id": "shot1", "kind": "image_to_video", "model": "fal-ai/kling-video/v3/pro/image-to-video", "prompt": "turns around", "image_from": "character", "duration_s": 7.07 },
                { "id": "shot2", "kind": "image_to_video", "model": "bytedance/seedance-2.5/us/image-to-video", "prompt": "turns, arms out", "image_from": "after", "duration_s": 2.0 }
            ],
            "edit": { "sequence": [ { "step": "shot1", "duration_s": 7.07 }, { "step": "shot2", "duration_s": 5.95 } ],
                      "captions": [ { "text": "watch me", "start_s": 0, "end_s": 7.07 } ] }
        }))
        .unwrap()
    }

    #[test]
    fn validates_and_renders() {
        let f = sample();
        f.validate().unwrap();
        assert_eq!(render(&f.steps[0].prompt, &f.values(&BTreeMap::new())), "a woman in her 20s in a bathroom");
        let mut o = BTreeMap::new();
        o.insert("character".into(), "a man in his 30s".into());
        assert_eq!(render(&f.steps[0].prompt, &f.values(&o)), "a man in his 30s in a bathroom");
    }

    #[test]
    fn adapters_map_fields_and_clamp_durations() {
        let f = sample();
        let k = build_input(&f.steps[2], "p", Some("http://img"), &f.format);
        assert_eq!(k["start_image_url"], "http://img");
        assert_eq!(k["duration"], "8"); // ceil(7.07)
        let s = build_input(&f.steps[3], "p", Some("http://img"), &f.format);
        assert_eq!(s["image_url"], "http://img");
        assert_eq!(s["duration"], "4"); // clamped up to Seedance minimum
        let n = build_input(&f.steps[0], "p", None, &f.format);
        assert_eq!(n["aspect_ratio"], "9:16");
    }

    #[test]
    fn rejects_broken_chains() {
        let mut f = sample();
        f.steps[2].image_from = Some("shot2".into());
        assert!(f.validate().is_err());
        let mut g = sample();
        g.edit.sequence[0].step = "character".into();
        assert!(g.validate().is_err());
    }

    #[test]
    fn exports_a_fal_workflow_with_refs_and_merge() {
        let w = sample().to_fal_workflow("t");
        let nodes = &w["contents"]["nodes"];
        assert_eq!(nodes["shot1"]["input"]["start_image_url"], "$character.images.0.url");
        assert_eq!(nodes["after"]["input"]["image_urls"][0], "$character.images.0.url");
        assert_eq!(nodes["character"]["input"]["prompt"], "a woman in her 20s in a bathroom");
        assert_eq!(nodes["merge"]["input"]["video_urls"][1], "$shot2.video.url");
        assert_eq!(w["contents"]["output"]["video"], "$merge.video.url");
    }

    /// The fal editor's rules: only run/display nodes, id == key, every `$x.`
    /// reference listed in `depends` (`$input.` → "input"), every schema input
    /// has a `modelId` that exists and reads it, output display depends on all.
    #[test]
    fn workflow_follows_fal_editor_rules() {
        fn refs(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::String(s) if s.starts_with('$') => out.push(s[1..].split('.').next().unwrap().to_string()),
                Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
                Value::Object(o) => o.values().for_each(|x| refs(x, out)),
                _ => {}
            }
        }
        let w = sample().to_fal_workflow("t");
        let c = &w["contents"];
        let nodes = c["nodes"].as_object().unwrap();
        assert_eq!(nodes["output"]["type"], "display");
        for (key, n) in nodes {
            assert_eq!(n["id"], *key);
            assert!(matches!(n["type"].as_str(), Some("run") | Some("display")), "{key}");
            let deps: Vec<&str> = n["depends"].as_array().unwrap().iter().map(|d| d.as_str().unwrap()).collect();
            let mut r = vec![];
            refs(&n["input"], &mut r);
            refs(&n["fields"], &mut r);
            for x in r {
                assert!(deps.contains(&x.as_str()), "{key} references ${x} without depends");
                assert!(x == "input" || nodes.contains_key(&x), "{key} → unknown node {x}");
            }
        }
        for (k, f) in c["schema"]["input"].as_object().unwrap() {
            assert_eq!(f["name"], *k);
            let m = f["modelId"].as_str().unwrap();
            assert!(nodes[m]["input"].to_string().contains(&format!("$input.{k}")), "{k} not read by {m}");
        }
        assert_eq!(c["output"], nodes["output"]["fields"]);
    }
}
