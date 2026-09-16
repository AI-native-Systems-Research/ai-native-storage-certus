//! CLI: replay a Qwen-Bailian usage trace through one or more `IEvictionPolicy`
//! implementations and report cache-hits (effectiveness) and mean per-call
//! latency (performance) for one or more cache sizes. The selected dataset is
//! downloaded to `/tmp` on first use.
//!
//! ```text
//! cargo run -p eviction-replay-benchmark -- \
//!     --dataset chat --cache-size-nelements 256K,1M,4M --policy both
//! ```

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
        }
        if cli.cache_sizes.len() > 1 {
            println!();
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
