//! build: run a flow on fal (dependency waves), download every output, and
//! assemble the final video locally (trim to the original cuts, captions, music).

use crate::fal::{self, Fal};
use crate::flow::{self, Flow, Kind};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::task::JoinSet;

#[derive(Debug, Clone, Serialize)]
pub struct CostLine {
    pub step: String,
    pub model: String,
    pub usd: f64,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub lines: Vec<CostLine>,
    pub total_usd: f64,
    pub source: String,
}

/// Per-run cost: live fal pricing when a key is available, else the rough table.
pub async fn estimate(flow: &Flow, fal: Option<&Fal>) -> Estimate {
    let rough: HashMap<String, f64> = flow.rough_cost().into_iter().map(|(id, _, usd)| (id, usd)).collect();
    let mut live: HashMap<String, (f64, String)> = HashMap::new();
    if let Some(fal) = fal {
        let models: Vec<String> = {
            let mut m: Vec<String> = flow.steps.iter().map(|s| s.model.clone()).collect();
            m.sort();
            m.dedup();
            m
        };
        if let Ok(prices) = fal.pricing(&models).await {
            for (ep, price, unit, _) in prices {
                live.insert(ep, (price, unit));
            }
        }
    }
    let mut lines = vec![];
    let mut all_live = !flow.steps.is_empty();
    for s in &flow.steps {
        if let Some((price, unit)) = live.get(&s.model) {
            let u = unit.to_lowercase();
            let qty = if u.contains("second") { s.duration_s.unwrap_or(5.0).ceil().max(flow::model_info(&s.model).map(|m| m.min_s).unwrap_or(0.0)) } else { 1.0 };
            lines.push(CostLine { step: s.id.clone(), model: s.model.clone(), usd: (price * qty * 100.0).round() / 100.0, basis: format!("${price} per {unit} × {qty}") });
        } else {
            all_live = false;
            lines.push(CostLine { step: s.id.clone(), model: s.model.clone(), usd: *rough.get(&s.id).unwrap_or(&0.0), basis: "rough table".into() });
        }
    }
    let total_usd = (lines.iter().map(|l| l.usd).sum::<f64>() * 100.0).round() / 100.0;
    Estimate { lines, total_usd, source: if all_live { "fal pricing API".into() } else if live.is_empty() { "rough table (set FAL_KEY for live prices)".into() } else { "mixed: fal pricing API + rough table".into() } }
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub step: String,
    pub model: String,
    pub url: String,
    pub file: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    pub out_dir: String,
    pub video: Option<String>,
    pub steps: Vec<StepResult>,
    pub estimate: Estimate,
    pub variables: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

pub async fn build(flow: &Flow, fal: &Fal, out_dir: &Path, overrides: &BTreeMap<String, String>, max_usd: f64, music: Option<&Path>, log: &(dyn Fn(&str) + Sync)) -> Result<BuildResult> {
    flow.validate()?;
    let est = estimate(flow, Some(fal)).await;
    if est.total_usd > max_usd {
        bail!("estimated ${:.2} per run ({}) is over --max-usd {:.2}; raise the budget or pick cheaper models", est.total_usd, est.source, max_usd);
    }
    std::fs::create_dir_all(out_dir.join("steps"))?;
    let vals = flow.values(overrides);
    let mut urls: HashMap<String, String> = HashMap::new();
    let mut results: Vec<StepResult> = vec![];
    let mut pending: Vec<&flow::Step> = flow.steps.iter().collect();

    while !pending.is_empty() {
        let (ready, rest): (Vec<&flow::Step>, Vec<&flow::Step>) = pending.into_iter().partition(|s| s.image_from.as_ref().map(|d| urls.contains_key(d)).unwrap_or(true));
        if ready.is_empty() {
            bail!("flow has a dependency that never resolves");
        }
        pending = rest;
        let mut set = JoinSet::new();
        for s in ready {
            let prompt = flow::render(&s.prompt, &vals);
            let image = s.image_from.as_ref().and_then(|d| urls.get(d)).cloned();
            let input = flow::build_input(s, &prompt, image.as_deref(), &flow.format);
            let fal = fal.clone();
            let step = s.clone();
            let timeout = if matches!(s.kind, Kind::ImageToVideo | Kind::TextToVideo) { 900 } else { 300 };
            log(&format!("→ {} · {}", s.id, s.model));
            set.spawn(async move {
                let out = fal.run(&step.model, &input, Duration::from_secs(timeout), |_| {}).await;
                (step, input, out)
            });
        }
        while let Some(joined) = set.join_next().await {
            let (step, input, out) = joined?;
            let out = out.with_context(|| format!("step `{}` failed", step.id))?;
            let url = fal::media_url(&out).with_context(|| format!("step `{}` returned no media: {}", step.id, out))?;
            let ext = if matches!(step.kind, Kind::ImageToVideo | Kind::TextToVideo) { "mp4" } else { "png" };
            let file = out_dir.join("steps").join(format!("{}.{ext}", step.id));
            fal::download(&url, &file).await?;
            log(&format!("  ✓ {} → {}", step.id, file.display()));
            urls.insert(step.id.clone(), url.clone());
            results.push(StepResult { step: step.id.clone(), model: step.model.clone(), url, file: file.to_string_lossy().into_owned(), input });
        }
    }

    let mut warnings = vec![];
    let clips: Vec<(PathBuf, f64)> = flow
        .edit
        .sequence
        .iter()
        .map(|c| (out_dir.join("steps").join(format!("{}.mp4", c.step)), c.duration_s))
        .collect();
    let final_path = out_dir.join("rebuild.mp4");
    let captions: Vec<flow::Caption> = flow.edit.captions.iter().map(|c| flow::Caption { text: flow::render(&c.text, &vals), ..c.clone() }).collect();
    let music_path = music.map(PathBuf::from).or_else(|| flow.edit.audio.music_path.as_ref().map(PathBuf::from));
    let video = match assemble(&clips, &flow.format, &captions, music_path.as_deref(), &final_path, out_dir, &mut warnings) {
        Ok(()) => Some(final_path.to_string_lossy().into_owned()),
        Err(e) => {
            warnings.push(format!("assembly failed: {e:#}"));
            None
        }
    };
    if flow.edit.audio.mode == "music" && music_path.is_none() {
        warnings.push(format!("original used music ({}). Add your own licensed track with --music <file>.", flow.edit.audio.note));
    }
    let res = BuildResult { out_dir: out_dir.to_string_lossy().into_owned(), video, steps: results, estimate: est, variables: vals, warnings };
    std::fs::write(out_dir.join("build.json"), serde_json::to_string_pretty(&res)?)?;
    Ok(res)
}

pub fn find_font() -> Option<String> {
    if let Ok(f) = std::env::var("REH_FONT") {
        return Some(f);
    }
    [
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial Bold.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
    ]
    .iter()
    .find(|p| Path::new(p).exists())
    .map(|p| p.to_string())
}

fn has_drawtext() -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-filters"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("drawtext")).unwrap_or(false)
}

/// Escape a path for use inside an ffmpeg filter argument.
fn filter_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

pub fn assemble(clips: &[(PathBuf, f64)], fmt: &flow::Format, captions: &[flow::Caption], music: Option<&Path>, out: &Path, work: &Path, warnings: &mut Vec<String>) -> Result<()> {
    if clips.is_empty() {
        bail!("nothing to assemble");
    }
    let (w, h) = (fmt.width.max(2) / 2 * 2, fmt.height.max(2) / 2 * 2);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    for (p, _) in clips {
        cmd.arg("-i").arg(p);
    }
    if let Some(m) = music {
        cmd.arg("-i").arg(m);
    }
    let mut graph = String::new();
    for (i, (_, d)) in clips.iter().enumerate() {
        graph.push_str(&format!("[{i}:v]trim=0:{d:.3},setpts=PTS-STARTPTS,scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps={:.3},setsar=1,format=yuv420p[v{i}];", fmt.fps));
    }
    for i in 0..clips.len() {
        graph.push_str(&format!("[v{i}]"));
    }
    graph.push_str(&format!("concat=n={}:v=1:a=0[base]", clips.len()));

    let mut last = "base".to_string();
    if !captions.is_empty() {
        match (find_font(), has_drawtext()) {
            (Some(font), true) => {
                for (i, c) in captions.iter().enumerate() {
                    let tf = work.join(format!("caption{i}.txt"));
                    std::fs::write(&tf, &c.text)?;
                    let size = match c.size.as_str() {
                        "small" => h as f64 * 0.024,
                        "large" => h as f64 * 0.05,
                        _ => h as f64 * 0.034,
                    };
                    let y = match c.position.as_str() {
                        "top" => format!("h*0.12"),
                        "bottom" => format!("h*0.78-text_h"),
                        _ => format!("(h-text_h)/2"),
                    };
                    graph.push_str(&format!(
                        ";[{last}]drawtext=fontfile='{}':textfile='{}':fontsize={:.0}:fontcolor=white:borderw={:.0}:bordercolor=black@0.85:x=(w-text_w)/2:y={y}:enable='between(t,{:.3},{:.3})'[c{i}]",
                        filter_path(Path::new(&font)),
                        filter_path(&tf),
                        size,
                        (size / 14.0).max(2.0),
                        c.start_s,
                        c.end_s
                    ));
                    last = format!("c{i}");
                }
            }
            (None, _) => warnings.push("no font found for captions (set REH_FONT=/path/to/font.ttf); rendered without captions".into()),
            (_, false) => warnings.push("this ffmpeg has no drawtext filter (needs freetype); rendered without captions".into()),
        }
    }
    let total: f64 = clips.iter().map(|c| c.1).sum();
    if music.is_some() {
        graph.push_str(&format!(";[{}:a]atrim=0:{total:.3},asetpts=PTS-STARTPTS,afade=t=out:st={:.2}:d=0.6[aout]", clips.len(), (total - 0.6).max(0.0)));
    }
    cmd.arg("-filter_complex").arg(&graph).args(["-map", &format!("[{last}]")]);
    if music.is_some() {
        cmd.args(["-map", "[aout]", "-c:a", "aac", "-b:a", "160k"]);
    } else {
        cmd.arg("-an");
    }
    cmd.args(["-c:v", "libx264", "-preset", "medium", "-crf", "19", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-t", &format!("{total:.3}")]).arg(out);
    let o = cmd.output().context("run ffmpeg")?;
    if !o.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
    }
    Ok(())
}

/// One variant per JSON line: {"character":"…","caption":"…"}.
pub fn read_variants(path: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = vec![];
    for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).with_context(|| format!("{}:{}: not JSON", path.display(), i + 1))?;
        let m = v.as_object().with_context(|| format!("{}:{}: expected an object", path.display(), i + 1))?;
        out.push(m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect());
    }
    Ok(out)
}

pub fn summary_json(r: &BuildResult) -> Value {
    json!({ "video": r.video, "out_dir": r.out_dir, "estimate_usd": r.estimate.total_usd, "estimate_source": r.estimate.source, "warnings": r.warnings, "steps": r.steps.iter().map(|s| json!({ "step": s.step, "url": s.url })).collect::<Vec<_>>() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_variants_jsonl() {
        let dir = std::env::temp_dir().join("reh-test-variants");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("v.jsonl");
        std::fs::write(&p, "{\"character\":\"a man\",\"days\":30}\n\n{\"character\":\"a woman\"}\n").unwrap();
        let v = read_variants(&p).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["days"], "30");
    }

    /// Real ffmpeg: two synthetic clips → trimmed, scaled, concatenated, captioned.
    #[test]
    fn assembles_clips_with_captions() {
        if crate::probe::check_tools().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join("reh-test-assemble");
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str, color: &str| {
            let p = dir.join(name);
            let ok = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("color=c={color}:s=640x360:d=3:r=24"), "-pix_fmt", "yuv420p"])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            p
        };
        let a = mk("a.mp4", "red");
        let b = mk("b.mp4", "blue");
        let fmt = flow::Format { aspect: "9:16".into(), width: 720, height: 1280, duration_s: 3.5, fps: 30.0 };
        let caps = vec![flow::Caption { text: "watch me: 3 months' work".into(), start_s: 0.0, end_s: 2.0, position: "center".into(), size: "medium".into() }];
        let out = dir.join("out.mp4");
        let mut warnings = vec![];
        assemble(&[(a, 2.0), (b, 1.5)], &fmt, &caps, None, &out, &dir, &mut warnings).unwrap();
        let p = crate::probe::probe(&out, &dir.join("frames")).unwrap();
        assert_eq!((p.width, p.height), (720, 1280));
        assert!((p.duration_s - 3.5).abs() < 0.15, "duration {}", p.duration_s);
        assert_eq!(p.shots.len(), 2, "red→blue cut should be detected");
        assert!((p.shots[1].start_s - 2.0).abs() < 0.1);
    }

    #[test]
    fn escapes_filter_paths() {
        assert_eq!(filter_path(Path::new("/a b/c:d's.ttf")), "/a b/c\\:d\\'s.ttf");
    }
}
