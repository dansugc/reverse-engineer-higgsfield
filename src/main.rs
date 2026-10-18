//! REH — Reverse Engineer Higgsfield.
//! Upload an AI video → get the fal.ai flow that rebuilds the format → scale it.

mod analyze;
mod build;
mod fal;
mod flow;
mod mcp;
mod probe;
mod verify;
mod web;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

pub const EXAMPLE_FLOW: &str = include_str!("../examples/before-after-transformation/flow.json");
pub const EXAMPLE_REPORT: &str = include_str!("../examples/before-after-transformation/REPORT.md");

#[derive(Parser)]
#[command(name = "reh", version, about = "Reverse engineer an AI video (Higgsfield, Kling, Seedance…) into a fal.ai flow you can rebuild and scale.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Investigate a video → flow.json + REPORT.md (+ fal workflow JSON)
    Analyze {
        video: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Video model used to read the clip (via fal openrouter/router/video)
        #[arg(long, default_value = analyze::DEFAULT_VIDEO_LLM)]
        model: String,
        /// Probe only (no key needed): shots, timing, keyframes, skeleton flow
        #[arg(long)]
        no_ai: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show the steps and the cost of one run (no spending)
    Plan {
        flow: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Rebuild the video on fal from a flow
    Build {
        flow: PathBuf,
        /// Override a variable: --set character="a man in his 30s"
        #[arg(long = "set", value_parser = parse_kv)]
        set: Vec<(String, String)>,
        #[arg(long, default_value_t = 5.0)]
        max_usd: f64,
        /// Your own licensed music track for the final cut
        #[arg(long)]
        music: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Scale the format: one rebuild per line of a JSONL variants file
    Batch {
        flow: PathBuf,
        #[arg(long)]
        vars: PathBuf,
        /// Budget for the whole batch
        #[arg(long, default_value_t = 20.0)]
        max_usd: f64,
        #[arg(long)]
        music: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Compare a rebuild with the original (structure + video-model judge)
    Verify {
        original: PathBuf,
        rebuild: PathBuf,
        #[arg(long)]
        flow: Option<PathBuf>,
        /// Write flow.v2.json with the judge's prompt fixes
        #[arg(long)]
        apply: bool,
        #[arg(long, default_value = analyze::DEFAULT_VIDEO_LLM)]
        model: String,
        #[arg(long)]
        no_ai: bool,
    },
    /// Export the flow as a native fal Workflow (and create it on fal)
    Publish {
        flow: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Only write workflow.fal.json
        #[arg(long)]
        dry_run: bool,
    },
    /// Check ffmpeg, fonts and the fal key
    Doctor,
    /// See a finished reverse-engineering without any key
    Demo,
    /// Local web app: upload a video, see the flow, rebuild it
    Web {
        #[arg(long, default_value_t = 4400)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// MCP server (stdio) for Claude Code, Codex, Cursor…
    Mcp,
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=').map(|(k, v)| (k.trim().to_string(), v.to_string())).ok_or_else(|| format!("expected key=value, got `{s}`"))
}

pub fn load_flow(path: &Path) -> Result<flow::Flow> {
    let f: flow::Flow = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?).with_context(|| format!("{} is not a valid flow", path.display()))?;
    f.validate()?;
    Ok(f)
}

pub fn run_dir(base: Option<PathBuf>, stem: &str) -> PathBuf {
    base.unwrap_or_else(|| {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        PathBuf::from("reh-runs").join(format!("{stem}-{ts}"))
    })
}

fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("{question} — non-interactive shell: re-run with --yes to proceed.");
        return Ok(false);
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn print_plan(f: &flow::Flow, est: &build::Estimate) {
    println!("{} — {}\n", f.title, f.format_summary);
    for s in &f.steps {
        let line = est.lines.iter().find(|l| l.step == s.id);
        println!(
            "  {:<12} {:<15} {:<34} {:>7}",
            s.id,
            format!("{:?}", s.kind),
            flow::model_info(&s.model).map(|m| m.label.to_string()).unwrap_or_else(|| s.model.clone()),
            line.map(|l| format!("${:.2}", l.usd)).unwrap_or_default()
        );
    }
    println!("\n  edit: {}", f.edit.sequence.iter().map(|c| format!("{} {:.2}s", c.step, c.duration_s)).collect::<Vec<_>>().join(" → "));
    println!("  per run: ${:.2} ({})", est.total_usd, est.source);
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Analyze { video, out, model, no_ai, json } => {
            let stem = video.file_stem().map(|s| s.to_string_lossy().chars().take(40).collect::<String>()).unwrap_or_else(|| "video".into());
            let dir = run_dir(out, &stem);
            std::fs::create_dir_all(&dir)?;
            let fal = if no_ai { None } else { fal::Fal::from_env().ok() };
            if !no_ai && fal.is_none() {
                eprintln!("note: FAL_KEY not set — running probe-only (add FAL_KEY to .env for the full reading)");
            }
            let mut log = |m: &str| eprintln!("{m}");
            let a = analyze::analyze(&video, &dir, fal.as_ref(), &model, &mut log).await?;
            let flow_path = dir.join("flow.json");
            std::fs::write(&flow_path, serde_json::to_string_pretty(&a.flow)?)?;
            std::fs::write(dir.join("probe.json"), serde_json::to_string_pretty(&a.probe)?)?;
            if let Some(r) = &a.reading {
                std::fs::write(dir.join("reading.json"), serde_json::to_string_pretty(r)?)?;
            }
            std::fs::write(dir.join("workflow.fal.json"), serde_json::to_string_pretty(&a.flow.to_fal_workflow(&a.flow.name))?)?;
            let report = analyze::report_md(&a, &flow_path.to_string_lossy());
            std::fs::write(dir.join("REPORT.md"), &report)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "dir": dir, "flow": a.flow }))?);
            } else {
                println!("{report}");
                eprintln!("\n✓ {}", dir.display());
            }
        }
        Cmd::Plan { flow: path, json } => {
            let f = load_flow(&path)?;
            let fal = fal::Fal::from_env().ok();
            let est = build::estimate(&f, fal.as_ref()).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&est)?);
            } else {
                print_plan(&f, &est);
            }
        }
        Cmd::Build { flow: path, set, max_usd, music, out, yes } => {
            let f = load_flow(&path)?;
            let fal = fal::Fal::from_env()?;
            let est = build::estimate(&f, Some(&fal)).await;
            print_plan(&f, &est);
            if !confirm(&format!("\nSpend about ${:.2} on fal to rebuild this?", est.total_usd), yes)? {
                bail!("cancelled — nothing was spent");
            }
            let overrides: BTreeMap<String, String> = set.into_iter().collect();
            let dir = run_dir(out, &format!("{}-build", f.name));
            let log = |m: &str| eprintln!("{m}");
            let r = build::build(&f, &fal, &dir, &overrides, max_usd, music.as_deref(), &log).await?;
            for w in &r.warnings {
                eprintln!("! {w}");
            }
            println!("{}", r.video.clone().unwrap_or_else(|| format!("{} (clips only, see build.json)", r.out_dir)));
            eprintln!("next: reh verify <original.mp4> {} --flow {}", r.video.clone().unwrap_or_default(), path.display());
        }
        Cmd::Batch { flow: path, vars, max_usd, music, out, yes } => {
            let f = load_flow(&path)?;
            let variants = build::read_variants(&vars)?;
            let fal = fal::Fal::from_env()?;
            let est = build::estimate(&f, Some(&fal)).await;
            let total = est.total_usd * variants.len() as f64;
            println!("{} variant(s) × ${:.2} ≈ ${total:.2} ({})", variants.len(), est.total_usd, est.source);
            if total > max_usd {
                bail!("batch estimate ${total:.2} is over --max-usd {max_usd:.2}");
            }
            if !confirm(&format!("Spend about ${total:.2} on fal?"), yes)? {
                bail!("cancelled — nothing was spent");
            }
            let base = run_dir(out, &format!("{}-batch", f.name));
            let log = |m: &str| eprintln!("{m}");
            for (i, v) in variants.iter().enumerate() {
                eprintln!("\n== variant {}/{} {:?}", i + 1, variants.len(), v);
                match build::build(&f, &fal, &base.join(format!("v{:02}", i + 1)), v, est.total_usd * 1.5 + 0.01, music.as_deref(), &log).await {
                    Ok(r) => println!("{}", r.video.unwrap_or(r.out_dir)),
                    Err(e) => eprintln!("! variant {} failed: {e:#}", i + 1),
                }
            }
        }
        Cmd::Verify { original, rebuild, flow: flow_path, apply, model, no_ai } => {
            let work = run_dir(None, "verify");
            std::fs::create_dir_all(&work)?;
            let structural = verify::structural(&original, &rebuild, &work)?;
            let flow = flow_path.as_ref().map(|p| load_flow(p)).transpose()?;
            let judge = match (no_ai, fal::Fal::from_env().ok(), &flow) {
                (false, Some(fal), Some(f)) => Some(verify::judge(&original, &rebuild, f, &fal, &model).await?),
                (false, None, _) => {
                    eprintln!("note: FAL_KEY not set — structural checks only");
                    None
                }
                (false, _, None) => {
                    eprintln!("note: pass --flow flow.json for the judge and fixes");
                    None
                }
                _ => None,
            };
            let v = verify::Verdict { structural, judge };
            println!("{}", verify::render(&v));
            if let (true, Some(j), Some(f), Some(p)) = (apply, &v.judge, &flow, &flow_path) {
                let (f2, applied) = verify::apply_fixes(f, j);
                let out = p.with_file_name("flow.v2.json");
                std::fs::write(&out, serde_json::to_string_pretty(&f2)?)?;
                eprintln!("✓ applied {} fix(es) → {}", applied.len(), out.display());
            }
        }
        Cmd::Publish { flow: path, name, dry_run } => {
            let f = load_flow(&path)?;
            let name = name.unwrap_or_else(|| format!("reh-{}", f.name)).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>();
            let wf = f.to_fal_workflow(&name);
            let out = path.with_file_name("workflow.fal.json");
            std::fs::write(&out, serde_json::to_string_pretty(&wf)?)?;
            println!("✓ wrote {}", out.display());
            if !dry_run {
                let fal = fal::Fal::from_env()?;
                match fal.create_workflow_unique(&name, |n| f.to_fal_workflow(n)).await {
                    Ok((n, url)) => println!("✓ created fal Workflow `{n}` {}", url.map(|u| format!("→ {u}")).unwrap_or_default()),
                    Err(e) => eprintln!("! {e:#}\n  The JSON above is ready to import in the fal Workflow editor."),
                }
            }
        }
        Cmd::Doctor => {
            let ok = |b: bool| if b { "✓" } else { "·" };
            println!("reh {} · harness {}", env!("CARGO_PKG_VERSION"), analyze::harness_sha());
            let tools = probe::check_tools().is_ok();
            println!("{} ffmpeg + ffprobe", ok(tools));
            println!("{} caption font {}", ok(build::find_font().is_some()), build::find_font().unwrap_or_else(|| "(none — set REH_FONT)".into()));
            match fal::Fal::from_env() {
                Ok(f) => match f.check_key().await {
                    Ok(()) => println!("✓ FAL_KEY works"),
                    Err(e) => println!("· FAL_KEY set but {e:#}"),
                },
                Err(_) => println!("· FAL_KEY not set — analyze runs probe-only; build/verify need it"),
            }
        }
        Cmd::Demo => {
            let f: flow::Flow = serde_json::from_str(EXAMPLE_FLOW)?;
            let dir = PathBuf::from("reh-runs/demo");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("flow.json"), EXAMPLE_FLOW)?;
            std::fs::write(dir.join("workflow.fal.json"), serde_json::to_string_pretty(&f.to_fal_workflow(&f.name))?)?;
            println!("{EXAMPLE_REPORT}");
            eprintln!("✓ demo flow written to {}/flow.json — try: reh plan {}/flow.json", dir.display(), dir.display());
        }
        Cmd::Web { port, no_open } => web::serve(port, !no_open).await?,
        Cmd::Mcp => mcp::serve().await?,
    }
    Ok(())
}
