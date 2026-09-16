//! CLI: replay a Qwen-Bailian usage trace through one or more `IEvictionPolicy`
//! implementations and report cache-hits (effectiveness) and mean per-call
//! latency (performance) for one or more cache sizes. The selected dataset is
//! downloaded to `/tmp` on first use.
//!
//! ```text
//! cargo run -p eviction-replay-benchmark -- \
//!     --dataset chat --cache-size-nelements 256K,1M,4M --policy both
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use component_core::query_interface;
use interfaces::IEvictionPolicy;

use eviction_replay_benchmark::dataset;
use eviction_replay_benchmark::replay;
use eviction_replay_benchmark::sim::{simulate, SimStats};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum PolicyArg {
    /// Recency-only LRU (`eviction-policy-lru`).
    #[value(name = "eviction-policy-lru", alias = "lru")]
    Lru,
    /// Session-lineage policy (`eviction-policy-session-lists`).
    #[value(name = "eviction-policy-session-lists", alias = "session-lists")]
    SessionLists,
    /// Run all policies and print them side by side.
    #[value(alias = "both")]
    All,
    /// Run all policies, print only the best hit rate per cache size.
    Best,
}

/// Which Qwen-Bailian trace to replay. Downloaded to `/tmp` on first use.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Dataset {
    /// To-C interactive chat, multi-turn (`qwen_traceA`).
    Chat,
    /// To-B API-driven task automation (`qwen_traceB`).
    Api,
    /// Reasoning-intensive chat (`qwen_thinking`).
    Thinking,
    /// Code generation (`qwen_coder`).
    Coder,
}

impl Dataset {
    fn id(self) -> &'static str {
        match self {
            Dataset::Chat => "chat",
            Dataset::Api => "api",
            Dataset::Thinking => "thinking",
            Dataset::Coder => "coder",
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum PolicyKind {
    Lru,
    SessionLists,
}

impl PolicyKind {
    fn label(self) -> &'static str {
        match self {
            PolicyKind::Lru => "lru",
            PolicyKind::SessionLists => "session-lists",
        }
    }

    /// Build a fresh component instance and run one replay.
    fn run(self, trace: &replay::Trace, cache_size: usize) -> SimStats {
        match self {
            PolicyKind::Lru => {
                let comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
                let ep = query_interface!(comp, IEvictionPolicy)
                    .expect("eviction-policy-lru provides IEvictionPolicy");
                simulate(&*ep, trace, cache_size)
            }
            PolicyKind::SessionLists => {
                let comp =
                    eviction_policy_session_lists::EvictionPolicySessionListsComponent::new_default(
                    );
                let ep = query_interface!(comp, IEvictionPolicy)
                    .expect("eviction-policy-session-lists provides IEvictionPolicy");
                simulate(&*ep, trace, cache_size)
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Replay a Qwen-Bailian usage trace through an IEvictionPolicy: cache-hits + latency"
)]
struct Cli {
    /// Which Qwen-Bailian trace to replay (downloaded to /tmp on first use).
    #[arg(long, value_enum, default_value_t = Dataset::Chat)]
    dataset: Dataset,

    /// Use a local Qwen-format JSONL file instead of downloading a dataset.
    #[arg(long)]
    qwen_file: Option<PathBuf>,

    /// Use a local ShareGPT-format JSON file (array of {id, conversations}).
    #[arg(long)]
    sharegpt: Option<PathBuf>,

    /// Use a local Weka JSONL trace file (one conversation per line, with
    /// hash_ids and hash_id_scope).
    #[arg(long)]
    weka_file: Option<PathBuf>,

    /// Characters per cache block when converting ShareGPT text to block keys
    /// (approximately 16 tokens at ~4 chars/token).
    #[arg(long, default_value_t = 64)]
    block_chars: usize,

    /// Cache size(s) in elements to evaluate (comma-separated). Accepts
    /// suffixes: K = ×1024, M = ×1024², G = ×1024³. Examples: 256K, 2M, 4096.
    #[arg(
        long = "cache-size-nelements",
        value_delimiter = ',',
        default_value = "10000",
        value_parser = parse_cache_size,
    )]
    cache_sizes: Vec<usize>,

    /// Maximum number of conversations to replay from the trace file.
    /// When set, only the first N conversations are loaded.
    #[arg(long)]
    max_conversations: Option<usize>,

    /// Which policy to run.
    #[arg(long, value_enum, default_value_t = PolicyArg::All)]
    policy: PolicyArg,

    /// Write a hit-rate-vs-cache-size plot to this PDF path.
    #[arg(long)]
    output_pdf: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Resolve the trace: --sharegpt, --weka-file, --qwen-file, or --dataset (download-on-demand).
    let (trace, source) = if let Some(ref p) = cli.sharegpt {
        match eviction_replay_benchmark::sharegpt::load(p, Some(cli.block_chars), cli.max_conversations) {
            Ok(t) => (
                t,
                format!("sharegpt {} (block_chars={})", p.display(), cli.block_chars),
            ),
            Err(e) => {
                eprintln!("error: failed to load ShareGPT file {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(ref p) = cli.weka_file {
        match eviction_replay_benchmark::weka::load(p, cli.max_conversations) {
            Ok(t) => (t, format!("weka {}", p.display())),
            Err(e) => {
                eprintln!("error: failed to load Weka trace {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        let (path, src) = match &cli.qwen_file {
            Some(p) => (p.clone(), format!("qwen-file {}", p.display())),
            None => match dataset::ensure(cli.dataset.id()) {
                Ok(p) => (
                    p,
                    format!(
                        "dataset {} ({})",
                        cli.dataset.id(),
                        dataset::describe(cli.dataset.id()).unwrap_or("")
                    ),
                ),
                Err(e) => {
                    eprintln!("error: could not obtain dataset: {e}");
                    return ExitCode::FAILURE;
                }
            },
        };
        let src = format!("{src}\n  file: {}", path.display());
        match replay::load(&path, cli.max_conversations) {
            Ok(t) => (t, src),
            Err(e) => {
                eprintln!("error: failed to load trace {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    };

    if trace.ops.is_empty() {
        eprintln!("error: trace has no key-bearing operations");
        return ExitCode::FAILURE;
    }

    let all_kinds: &[PolicyKind] = &[PolicyKind::Lru, PolicyKind::SessionLists];
    let kinds: &[PolicyKind] = match cli.policy {
        PolicyArg::Lru => &[PolicyKind::Lru],
        PolicyArg::SessionLists => &[PolicyKind::SessionLists],
        PolicyArg::All | PolicyArg::Best => all_kinds,
    };
    let best_only = cli.policy == PolicyArg::Best;

    println!("{source}");
    println!(
        "  requests={}  accesses(block-refs)={}  working-set(distinct blocks)={}",
        trace.ops.len(),
        trace.total_key_refs,
        trace.distinct_keys
    );
    println!(
        "  effectiveness = hit% (higher keeps important blocks longer); \
         performance = mean touch / evict latency"
    );
    println!();

    println!(
        "{:<14} {:>7} {:>9} {:>7} {:>9} {:>12} {:>12} {:>12} {:>12}",
        "policy", "cache", "hits", "hit%", "evicts", "touch(ns)", "evict(ns)", "track(ns)", "ops/s"
    );
    println!("{}", "-".repeat(100));

    let mut all_results: Vec<(usize, PolicyKind, SimStats)> = Vec::new();

    for &size in &cli.cache_sizes {
        let mut results: Vec<(PolicyKind, SimStats)> = kinds
            .iter()
            .map(|&kind| (kind, kind.run(&trace, size)))
            .collect();

        if best_only {
            results.sort_by(|a, b| b.1.hit_rate().partial_cmp(&a.1.hit_rate()).unwrap());
            results.truncate(1);
        }

        for (kind, s) in &results {
            println!(
                "{:<14} {:>7} {:>9} {:>6.1}% {:>9} {:>12.1} {:>12.1} {:>12.1} {:>12}",
                kind.label(),
                format_size(size),
                s.hits,
                s.hit_rate() * 100.0,
                s.evictions,
                s.mean_touch_ns(),
                s.mean_evict_ns(),
                s.mean_track_ns(),
                format_thousands(s.ops_per_sec() as u64),
            );
            all_results.push((size, *kind, s.clone()));
        }
        if cli.cache_sizes.len() > 1 {
            println!();
        }
    }

    if let Some(ref pdf_path) = cli.output_pdf {
        if let Err(e) = generate_pdf(&all_results, pdf_path, &source) {
            eprintln!("warning: failed to generate PDF: {e}");
        }
    }

    ExitCode::SUCCESS
}

fn parse_cache_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let (num_part, multiplier) = if let Some(n) = s.strip_suffix('G') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1024)
    } else {
        (s, 1)
    };
    let num: f64 = num_part
        .trim()
        .parse()
        .map_err(|e| format!("invalid cache size '{s}': {e}"))?;
    let result = (num * multiplier as f64) as usize;
    if result == 0 {
        return Err(format!("cache size must be > 0, got '{s}'"));
    }
    Ok(result)
}

fn format_size(n: usize) -> String {
    if n >= 1024 * 1024 * 1024 && n % (1024 * 1024 * 1024) == 0 {
        format!("{}G", n / (1024 * 1024 * 1024))
    } else if n >= 1024 * 1024 && n % (1024 * 1024) == 0 {
        format!("{}M", n / (1024 * 1024))
    } else if n >= 1024 && n % 1024 == 0 {
        format!("{}K", n / 1024)
    } else {
        n.to_string()
    }
}

fn generate_pdf(
    results: &[(usize, PolicyKind, SimStats)],
    pdf_path: &std::path::Path,
    title: &str,
) -> Result<(), String> {
    let mut policies: std::collections::BTreeMap<&str, Vec<(usize, f64)>> =
        std::collections::BTreeMap::new();
    for (size, kind, stats) in results {
        policies
            .entry(kind.label())
            .or_default()
            .push((*size, stats.hit_rate() * 100.0));
    }

    let mut json_series = String::from("[");
    for (i, (label, points)) in policies.iter().enumerate() {
        if i > 0 {
            json_series.push(',');
        }
        let xs: Vec<String> = points.iter().map(|(s, _)| s.to_string()).collect();
        let ys: Vec<String> = points.iter().map(|(_, h)| format!("{h:.2}")).collect();
        json_series.push_str(&format!(
            "{{\"label\":\"{label}\",\"x\":[{}],\"y\":[{}]}}",
            xs.join(","),
            ys.join(",")
        ));
    }
    json_series.push(']');

    let title_line = title.lines().next().unwrap_or(title);
    let pdf_str = pdf_path.display().to_string();

    let mut script_file = tempfile::NamedTempFile::new()
        .map_err(|e| format!("failed to create temp script: {e}"))?;
    script_file
        .write_all(PLOT_SCRIPT.as_bytes())
        .map_err(|e| format!("failed to write temp script: {e}"))?;

    let output = std::process::Command::new("python3")
        .args([
            script_file.path().as_os_str(),
            std::ffi::OsStr::new(&json_series),
            std::ffi::OsStr::new(&pdf_str),
            std::ffi::OsStr::new(title_line),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run python3: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "python3 exited {}: {}",
            output.status,
            stderr.trim_end()
        ));
    }

    eprintln!("wrote {pdf_str}");
    Ok(())
}

const PLOT_SCRIPT: &str = r##"
import json, sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

series = json.loads(sys.argv[1])
pdf_path = sys.argv[2]
title = sys.argv[3]

colors = ["#2563eb", "#dc2626", "#16a34a", "#9333ea", "#ea580c"]
markers = ["o", "s", "^", "D", "v"]
fig, ax = plt.subplots(figsize=(8, 5))
for i, s in enumerate(series):
    c = colors[i % len(colors)]
    m = markers[i % len(markers)]
    ax.plot(s["x"], s["y"], f"{m}-", color=c, linewidth=2, markersize=7, label=s["label"])
    for x, y in zip(s["x"], s["y"]):
        ax.annotate(f"{y:.1f}%", (x, y), textcoords="offset points",
                    xytext=(0, 10 if i == 0 else -15), ha="center", fontsize=8, color=c)
ax.set_xlabel("Cache Size (elements)", fontsize=12)
ax.set_ylabel("Hit Rate (%)", fontsize=12)
ax.set_title(title, fontsize=13)
ax.legend(fontsize=11)
ax.grid(True, alpha=0.3)
if series:
    all_x = sorted(set(x for s in series for x in s["x"]))
    ax.set_xticks(all_x)
    labels = []
    for v in all_x:
        if v >= 1024*1024*1024 and v % (1024*1024*1024) == 0:
            labels.append(f"{v//(1024*1024*1024)}G")
        elif v >= 1024*1024 and v % (1024*1024) == 0:
            labels.append(f"{v//(1024*1024)}M")
        elif v >= 1024 and v % 1024 == 0:
            labels.append(f"{v//1024}K")
        else:
            labels.append(str(v))
    ax.set_xticklabels(labels)
fig.tight_layout()
fig.savefig(pdf_path, dpi=150)
"##;

/// Format an integer with `_` thousands separators for readability.
fn format_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push('_');
        }
        out.push(b as char);
    }
    out
}
