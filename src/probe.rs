//! Deterministic, local evidence from the video file itself (ffprobe/ffmpeg).
//! Everything here is an *observation*: no model is involved.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Probe {
    pub file: String,
    pub bytes: u64,
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub aspect: String,
    pub has_audio: bool,
    pub audio_mean_db: Option<f64>,
    pub container_tags: serde_json::Map<String, serde_json::Value>,
    /// Hints about where the file came from (TikTok re-encode, C2PA manifest, generator tags…).
    pub provenance: Vec<String>,
    pub shots: Vec<Shot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    pub index: usize,
    pub start_s: f64,
    pub end_s: f64,
    pub duration_s: f64,
    /// Keyframes extracted at ~15%, 50%, 85% of the shot.
    pub keyframes: Vec<String>,
}

fn run(cmd: &mut Command) -> Result<std::process::Output> {
    let out = cmd.output().with_context(|| format!("failed to run {:?} (is ffmpeg installed?)", cmd.get_program()))?;
    Ok(out)
}

pub fn check_tools() -> Result<()> {
    for bin in ["ffmpeg", "ffprobe"] {
        if Command::new(bin).arg("-version").output().map(|o| !o.status.success()).unwrap_or(true) {
            bail!("{bin} not found on PATH — install ffmpeg (brew install ffmpeg / apt install ffmpeg)");
        }
    }
    Ok(())
}

fn ratio(w: u32, h: u32) -> String {
    let r = w as f64 / h as f64;
    let known = [(9.0 / 16.0, "9:16"), (16.0 / 9.0, "16:9"), (1.0, "1:1"), (4.0 / 5.0, "4:5"), (3.0 / 4.0, "3:4"), (4.0 / 3.0, "4:3"), (21.0 / 9.0, "21:9"), (2.0 / 3.0, "2:3")];
    known
        .iter()
        .min_by(|a, b| (a.0 - r).abs().partial_cmp(&(b.0 - r).abs()).unwrap())
        .map(|k| k.1.to_string())
        .unwrap_or_else(|| format!("{w}:{h}"))
}

/// Scene-cut timestamps via ffmpeg's scene score.
pub fn scene_cuts(file: &Path, threshold: f64) -> Result<Vec<f64>> {
    let out = run(Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-vf", &format!("select='gt(scene,{threshold})',showinfo"), "-an", "-f", "null", "-"]))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut cuts = vec![];
    for line in stderr.lines() {
        if let Some(i) = line.find("pts_time:") {
            let rest = &line[i + 9..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(t) = num.parse::<f64>() {
                cuts.push(t);
            }
        }
    }
    Ok(cuts)
}

/// Turn cut points into shots, dropping slivers shorter than `min_shot`.
pub fn shots_from_cuts(duration: f64, cuts: &[f64], min_shot: f64) -> Vec<(f64, f64)> {
    let mut bounds = vec![0.0];
    for &c in cuts {
        if c - bounds.last().copied().unwrap_or(0.0) >= min_shot && duration - c >= min_shot {
            bounds.push(c);
        }
    }
    bounds.push(duration);
    bounds.windows(2).map(|w| (w[0], w[1])).collect()
}

pub fn extract_frame(file: &Path, t: f64, out: &Path) -> Result<()> {
    let o = run(Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", "scale='min(720,iw)':-2", "-q:v", "3"])
        .arg(out))?;
    if !o.status.success() {
        bail!("frame extraction failed at {t:.2}s: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(())
}

fn mean_volume(file: &Path) -> Option<f64> {
    let o = Command::new("ffmpeg").args(["-hide_banner", "-nostats", "-i"]).arg(file).args(["-vn", "-af", "volumedetect", "-f", "null", "-"]).output().ok()?;
    let s = String::from_utf8_lossy(&o.stderr);
    let line = s.lines().find(|l| l.contains("mean_volume:"))?;
    line.split("mean_volume:").nth(1)?.trim().trim_end_matches(" dB").parse().ok()
}

fn provenance(file: &Path, tags: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut hints = vec![];
    if tags.contains_key("aigc_info") || tags.get("comment").and_then(|v| v.as_str()).map(|c| c.starts_with("vid:")).unwrap_or(false) {
        hints.push("TikTok re-encode (aigc_info / vid tags): original generator metadata was stripped".into());
    }
    if let Some(enc) = tags.get("encoder").and_then(|v| v.as_str()) {
        hints.push(format!("encoder: {enc}"));
    }
    // Look for C2PA / generator markers in the first and last MB.
    if let Ok(bytes) = std::fs::read(file) {
        let scan = |b: &[u8]| -> Vec<&'static str> {
            let hay = String::from_utf8_lossy(b).to_lowercase();
            ["c2pa", "jumbf", "higgsfield", "kling", "seedance", "runway", "veo", "hailuo", "minimax", "sora", "pika", "luma"]
                .into_iter()
                .filter(|k| hay.contains(*k))
                .collect()
        };
        let n = bytes.len();
        let head = &bytes[..n.min(1 << 20)];
        let tail = &bytes[n.saturating_sub(1 << 20)..];
        let mut found = scan(head);
        found.extend(scan(tail));
        found.sort();
        found.dedup();
        // "veo"/"luma" etc. can appear in random compressed bytes; only trust with a container hint.
        let strong: Vec<_> = found.iter().filter(|k| ["c2pa", "jumbf", "higgsfield"].contains(k)).collect();
        if !strong.is_empty() {
            hints.push(format!("embedded markers: {}", strong.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", ")));
        } else {
            hints.push("no C2PA / generator manifest found".into());
        }
    }
    hints
}

pub fn probe(file: &Path, frames_dir: &Path) -> Result<Probe> {
    check_tools()?;
    let out = run(Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(file))?;
    if !out.status.success() {
        bail!("ffprobe could not read {}: {}", file.display(), String::from_utf8_lossy(&out.stderr));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").context("no video stream")?;
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let width = video["width"].as_u64().unwrap_or(0) as u32;
    let height = video["height"].as_u64().unwrap_or(0) as u32;
    let fps = video["r_frame_rate"]
        .as_str()
        .and_then(|r| {
            let (a, b) = r.split_once('/')?;
            Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?.max(1.0))
        })
        .unwrap_or(30.0);
    let duration_s: f64 = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let tags = v["format"]["tags"].as_object().cloned().unwrap_or_default();

    let cuts = scene_cuts(file, 0.3)?;
    std::fs::create_dir_all(frames_dir)?;
    let mut shots = vec![];
    for (i, (a, b)) in shots_from_cuts(duration_s, &cuts, 0.6).into_iter().enumerate() {
        let mut keyframes = vec![];
        for (j, f) in [0.15, 0.5, 0.85].iter().enumerate() {
            let t = a + (b - a) * f;
            let name = format!("shot{:02}_{}.jpg", i + 1, ["start", "mid", "end"][j]);
            let p: PathBuf = frames_dir.join(&name);
            extract_frame(file, t, &p)?;
            keyframes.push(p.to_string_lossy().into_owned());
        }
        shots.push(Shot { index: i + 1, start_s: round(a), end_s: round(b), duration_s: round(b - a), keyframes });
    }

    Ok(Probe {
        file: file.to_string_lossy().into_owned(),
        bytes: std::fs::metadata(file)?.len(),
        duration_s: round(duration_s),
        width,
        height,
        fps: round(fps),
        aspect: ratio(width, height),
        has_audio,
        audio_mean_db: if has_audio { mean_volume(file) } else { None },
        provenance: provenance(file, &tags),
        container_tags: tags,
        shots,
    })
}

pub fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_become_shots_and_slivers_are_dropped() {
        let s = shots_from_cuts(13.0, &[0.2, 7.07, 12.9], 0.6);
        assert_eq!(s, vec![(0.0, 7.07), (7.07, 13.0)]);
        assert_eq!(shots_from_cuts(5.0, &[], 0.6), vec![(0.0, 5.0)]);
    }

    #[test]
    fn aspect_ratio_names() {
        assert_eq!(ratio(720, 1280), "9:16");
        assert_eq!(ratio(1920, 1080), "16:9");
        assert_eq!(ratio(1080, 1350), "4:5");
    }
}
