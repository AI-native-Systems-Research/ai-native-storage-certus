//! CLI: replay a Qwen-Bailian usage trace through one or more `IEvictionPolicy`
//! implementations and report cache-hits (effectiveness) and mean per-call
//! latency (performance) for one or more cache sizes. The selected dataset is
//! downloaded to `/tmp` on first use.
//!
//! ```text
//! cargo run -p eviction-replay-benchmark -- \
//!     --dataset chat --cache-size 64,256,1024 --policy both
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
    Lru,
    /// Session-lineage policy (`eviction-policy-session-lists`).
    SessionLists,
    /// Run both and print them side by side (default).
    Both,
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
    file: Option<PathBuf>,

    /// Cache size(s) in blocks to evaluate (comma-separated or repeated).
    #[arg(
        long = "cache-size",
        value_delimiter = ',',
        default_value = "256,1024,4096"
    )]
    cache_sizes: Vec<usize>,

    /// Which policy to run.
    #[arg(long, value_enum, default_value_t = PolicyArg::Both)]
    policy: PolicyArg,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Resolve the trace file: an explicit --file, else download-on-demand.
    let (path, source) = match &cli.file {
        Some(p) => (p.clone(), format!("file {}", p.display())),
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

    let trace = match replay::load(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to load trace {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    if trace.ops.is_empty() {
        eprintln!(
            "error: trace {} has no key-bearing operations",
            path.display()
        );
        return ExitCode::FAILURE;
    }

    let kinds: &[PolicyKind] = match cli.policy {
        PolicyArg::Lru => &[PolicyKind::Lru],
        PolicyArg::SessionLists => &[PolicyKind::SessionLists],
        PolicyArg::Both => &[PolicyKind::Lru, PolicyKind::SessionLists],
    };

    println!("{source}");
    println!("  file: {}", path.display());
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
        for &kind in kinds {
            let s = kind.run(&trace, size);
            println!(
                "{:<14} {:>7} {:>9} {:>6.1}% {:>9} {:>12.1} {:>12.1} {:>12.1} {:>12}",
                kind.label(),
                size,
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
