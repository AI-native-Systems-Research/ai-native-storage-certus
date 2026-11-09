"""Statistics collection and reporting."""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Metrics:
    """Collects simulation statistics for final reporting."""

    # Populate
    populate_count: int = 0
    populate_success: int = 0
    populate_latencies_us: list[float] = field(default_factory=list)

    # Lookup
    lookup_count: int = 0
    lookup_hot_hits: int = 0
    lookup_cold_hits: int = 0
    lookup_misses: int = 0
    lookup_hot_latencies_us: list[float] = field(default_factory=list)
    lookup_cold_latencies_us: list[float] = field(default_factory=list)

    # Remove
    remove_count: int = 0
    remove_success: int = 0

    # Touch
    touch_count: int = 0

    # Eviction
    eviction_clean: int = 0
    eviction_dirty: int = 0  # data loss evictions
    ssd_evictions: int = 0

    # Write-through
    write_through_success: int = 0
    write_through_failed: int = 0

    # Prepare/commit store (direct-write path)
    prepare_store_count: int = 0
    prepare_store_success: int = 0
    prepare_store_latencies_us: list[float] = field(default_factory=list)
    commit_store_count: int = 0
    commit_store_success: int = 0
    commit_store_latencies_us: list[float] = field(default_factory=list)
    cancel_store_count: int = 0

    # Promotion (SSD->DRAM)
    promotion_count: int = 0

    def record_populate(self, latency_us: float, success: bool) -> None:
        self.populate_count += 1
        if success:
            self.populate_success += 1
            self.populate_latencies_us.append(latency_us)

    def record_lookup(self, latency_us: float, hot: bool, success: bool) -> None:
        self.lookup_count += 1
        if not success:
            self.lookup_misses += 1
            return
        if hot:
            self.lookup_hot_hits += 1
            self.lookup_hot_latencies_us.append(latency_us)
        else:
            self.lookup_cold_hits += 1
            self.lookup_cold_latencies_us.append(latency_us)

    def record_remove(self, success: bool) -> None:
        self.remove_count += 1
        if success:
            self.remove_success += 1

    def record_touch(self) -> None:
        self.touch_count += 1

    def record_eviction(self, clean: bool) -> None:
        if clean:
            self.eviction_clean += 1
        else:
            self.eviction_dirty += 1

    def record_ssd_eviction(self) -> None:
        self.ssd_evictions += 1

    def record_write_through(self, success: bool) -> None:
        if success:
            self.write_through_success += 1
        else:
            self.write_through_failed += 1

    def record_prepare_store(self, latency_us: float, success: bool) -> None:
        self.prepare_store_count += 1
        if success:
            self.prepare_store_success += 1
            self.prepare_store_latencies_us.append(latency_us)

    def record_commit_store(self, latency_us: float, success: bool) -> None:
        self.commit_store_count += 1
        if success:
            self.commit_store_success += 1
            self.commit_store_latencies_us.append(latency_us)

    def record_cancel_store(self) -> None:
        self.cancel_store_count += 1

    def record_promotion(self) -> None:
        self.promotion_count += 1

    def hit_rate(self) -> float:
        total_lookups = self.lookup_hot_hits + self.lookup_cold_hits + self.lookup_misses
        if total_lookups == 0:
            return 0.0
        return (self.lookup_hot_hits + self.lookup_cold_hits) / total_lookups

    def hot_hit_rate(self) -> float:
        total_hits = self.lookup_hot_hits + self.lookup_cold_hits + self.lookup_misses
        if total_hits == 0:
            return 0.0
        return self.lookup_hot_hits / total_hits

    def _percentile(self, data: list[float], p: float) -> float:
        if not data:
            return 0.0
        sorted_data = sorted(data)
        idx = int(len(sorted_data) * p / 100.0)
        idx = min(idx, len(sorted_data) - 1)
        return sorted_data[idx]

    def summary(self) -> str:
        lines = [
            "=" * 60,
            "  CERTUS SIMULATOR RESULTS",
            "=" * 60,
            "",
            "-- Populate --",
            f"  Total:       {self.populate_count}",
            f"  Success:     {self.populate_success}",
            f"  Failed:      {self.populate_count - self.populate_success}",
        ]
        if self.populate_latencies_us:
            lines += [
                f"  Avg latency: {sum(self.populate_latencies_us)/len(self.populate_latencies_us):.1f} us",
                f"  P50 latency: {self._percentile(self.populate_latencies_us, 50):.1f} us",
                f"  P99 latency: {self._percentile(self.populate_latencies_us, 99):.1f} us",
            ]

        lines += [
            "",
            "-- Lookup --",
            f"  Total:       {self.lookup_count}",
            f"  Hot hits:    {self.lookup_hot_hits}  (staging + memory-tier)",
            f"  Cold hits:   {self.lookup_cold_hits}  (SSD promotion)",
            f"  Misses:      {self.lookup_misses}",
            f"  Hit rate:    {self.hit_rate()*100:.1f}%",
            f"  Hot hit rate:{self.hot_hit_rate()*100:.1f}%",
        ]
        if self.lookup_hot_latencies_us:
            lines += [
                f"  Hot avg:     {sum(self.lookup_hot_latencies_us)/len(self.lookup_hot_latencies_us):.1f} us",
                f"  Hot P50:     {self._percentile(self.lookup_hot_latencies_us, 50):.1f} us",
                f"  Hot P99:     {self._percentile(self.lookup_hot_latencies_us, 99):.1f} us",
            ]
        if self.lookup_cold_latencies_us:
            lines += [
                f"  Cold avg:    {sum(self.lookup_cold_latencies_us)/len(self.lookup_cold_latencies_us):.1f} us",
                f"  Cold P50:    {self._percentile(self.lookup_cold_latencies_us, 50):.1f} us",
                f"  Cold P99:    {self._percentile(self.lookup_cold_latencies_us, 99):.1f} us",
            ]

        lines += [
            "",
            "-- Remove --",
            f"  Total:       {self.remove_count}",
            f"  Success:     {self.remove_success}",
            "",
            "-- Touch --",
            f"  Total:       {self.touch_count}",
            "",
            "-- Direct-Write (prepare_store/commit_store) --",
            f"  Prepared:    {self.prepare_store_count} ({self.prepare_store_success} success)",
            f"  Committed:   {self.commit_store_count} ({self.commit_store_success} success)",
            f"  Cancelled:   {self.cancel_store_count}",
        ]
        if self.commit_store_latencies_us:
            lines += [
                f"  Commit avg:  {sum(self.commit_store_latencies_us)/len(self.commit_store_latencies_us):.1f} us",
                f"  Commit P99:  {self._percentile(self.commit_store_latencies_us, 99):.1f} us",
            ]

        lines += [
            "",
            "-- Eviction --",
            f"  Memory-tier clean:  {self.eviction_clean}",
            f"  Memory-tier dirty:  {self.eviction_dirty} (data loss)",
            f"  SSD evictions:      {self.ssd_evictions}",
            "",
            "-- Write-Through --",
            f"  Success:     {self.write_through_success}",
            f"  Failed:      {self.write_through_failed}",
            "",
            "-- Promotion (SSD->DRAM) --",
            f"  Promoted:    {self.promotion_count}",
            "",
            "=" * 60,
        ]
        return "\n".join(lines)
