# Expected Hit Ratios for KV-Cache Deployments

## Workload Regimes

| Workload Pattern | Expected Hit Ratio | Key Driver |
|---|---|---|
| High prefix sharing (chatbot/agent, shared system prompts, many users) | 50–80%+ | Shared system prompt and few-shot prefix reused across requests; only per-request suffix misses |
| Multi-turn conversations | 70–80% (grows with turn count) | Each turn reuses prior context; first turn is always a miss |
| Diverse/unique prompts (varied RAG, one-shot, no prefix overlap) | 10–30% | Little reuse; cache churns. Worst case for SSD tier |

## Planning Target

For a typical production mix (shared system prompt + multi-turn conversations), **60–75%** is a reasonable planning target.

## Key Factors

- **Prefix deduplication granularity**: Block-level prefix matching (rather than exact full-key match) significantly improves hit ratios.
- **Reference point**: Systems like vLLM's automatic prefix caching report 2–5x throughput gains on shared-prefix workloads, implying effective hit ratios of 50–80%.
- **Conversation length**: Longer multi-turn conversations yield higher per-session hit ratios since each new turn adds only incremental KV entries on top of cached context.

## Consideration

Hit-ratio telemetry should be included so deployments can monitor whether the SSD tier is earning its keep.
