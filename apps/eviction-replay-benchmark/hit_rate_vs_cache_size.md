# Hit Rate vs Cache Size — Weka Trace (50 conversations)

**Trace:** `/mnt/certus1/cc-traces-weka-062126.jsonl` (50 conversations, 7,291 requests, 38.4M accesses, 427K distinct blocks)

| Cache Size | LRU Hit% | Session-Lists Hit% | Delta (SL − LRU) |
|------------|----------|-------------------|-------------------|
| 6,000      | 35.2%    | 49.2%             | +14.0 pp          |
| 7,000      | 43.9%    | 53.6%             | +9.7 pp           |
| 8,000      | 52.7%    | 57.3%             | +4.6 pp           |
| 9,000      | 61.9%    | 60.4%             | −1.5 pp           |
| 10,000     | 70.6%    | 62.9%             | −7.7 pp           |
| 11,000     | 79.3%    | 65.0%             | −14.3 pp          |
| 12,000     | 88.6%    | 66.6%             | −22.0 pp          |

**Crossover:** Session-lists leads at small caches (≤8K) where lineage protection prevents prefix eviction. LRU overtakes at ~9K elements as the cache grows large enough that recency alone retains most active blocks, and session-lists' lineage constraints become counterproductive.
