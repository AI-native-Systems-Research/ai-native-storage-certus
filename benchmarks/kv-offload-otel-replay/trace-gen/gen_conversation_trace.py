#!/usr/bin/env python3.12
"""Run inference-perf's ConversationReplayDataGenerator and dump the trace to JSON.

conversation_replay has no built-in trace export, so we instantiate the real
generator and serialise its per-conversation blueprints. The random per-turn
prompt *text* is skipped on purpose: for 10k conversations it would be ~1.5B
tokens of meaningless random filler (many GB). The trace records the workload
STRUCTURE that actually matters — turn counts, per-turn input/output token
lengths, and inter-turn (tool-call) latencies — fully reproducible from
(seed, config).

Per-turn input token lengths are not stored on the blueprint (they are consumed
only to build the skipped text), so we capture them faithfully by recording the
exact token counts the generator requests, in call order:
    [shared_prompt] , then per conversation: [dynamic_prompt] + [turn_0 .. turn_k]

Usage:
    python3.12 gen_conversation_trace.py <config.yaml> <out.json> [num_conversations]
"""
import json
import sys
import time
import warnings

warnings.filterwarnings("ignore")

import numpy as np

from inference_perf.config import read_config
from inference_perf.datagen.conversation_replay_datagen import ConversationReplayDataGenerator
from inference_perf.utils.custom_tokenizer import CustomTokenizer
from inference_perf.config.utils import CustomTokenizerConfig


def main() -> None:
    config_path = sys.argv[1]
    out_path = sys.argv[2]
    num_conv = int(sys.argv[3]) if len(sys.argv) > 3 else None

    cfg = read_config(config_path)
    if num_conv is not None:
        cfg.data.conversation_replay.num_conversations = num_conv
    cr = cfg.data.conversation_replay

    # Capture every requested token length in call order, and skip the actual
    # (expensive, semantically-empty) random-text materialisation.
    requested_lens: list[int] = []

    def _capture(self, n, rng=None):  # replaces _generate_random_token_text
        requested_lens.append(int(n))
        return ""

    ConversationReplayDataGenerator._generate_random_token_text = _capture

    # A tokenizer object is required to construct the generator; vocab is only
    # used by the (skipped) text path, so a tiny local tokenizer is fine.
    tok = CustomTokenizer(CustomTokenizerConfig(pretrained_model_name_or_path="sshleifer/tiny-gpt2"))

    t0 = time.time()
    gen = ConversationReplayDataGenerator(cfg.api, cfg.data, tok)
    build_s = time.time() - t0

    # Walk requested_lens to recover per-turn input lengths.
    #   idx 0            : shared system prompt  (== shared_system_prompt_len)
    #   then per conv    : 1 dynamic-prompt call, then num_turns turn-prompt calls
    idx = 0
    shared_len = requested_lens[idx]
    idx += 1
    assert shared_len == cr.shared_system_prompt_len, (shared_len, cr.shared_system_prompt_len)

    conversations = []
    all_turns, all_in, all_out, all_lat = [], [], [], []
    for bp in gen.blueprints:
        idx += 1  # skip the per-conversation dynamic-prompt call
        input_lens = requested_lens[idx : idx + bp.num_turns]
        idx += bp.num_turns

        turns = []
        for i in range(bp.num_turns):
            inp = int(input_lens[i])
            out = int(bp.turn_output_lens[i])
            lat = float(bp.turn_tool_call_latencies[i]) if bp.turn_tool_call_latencies else 0.0
            turns.append([inp, out, lat])
            all_in.append(inp)
            all_out.append(out)
            all_lat.append(lat)
        all_turns.append(bp.num_turns)
        conversations.append(
            {
                "id": bp.conversation_id,
                "num_turns": bp.num_turns,
                "system_prompt_tokens": bp.system_prompt_tokens,
                "turns": turns,  # each: [input_tokens, output_tokens, tool_call_latency_sec]
            }
        )

    def dist_summary(d):
        if d is None:
            return None
        return {"type": d.type.value, "min": d.min, "max": d.max, "mean": d.mean, "std_dev": d.std_dev, "skew": d.skew}

    total_turns = int(sum(all_turns))
    trace = {
        "metadata": {
            "generator": "conversation_replay",
            "source_config": config_path,
            "seed": cr.seed,
            "num_conversations": cr.num_conversations,
            "shared_system_prompt_len": cr.shared_system_prompt_len,
            "note": "Workload PLAN only; random prompt filler text omitted by design. Reproducible from (seed, config).",
            "turn_fields": ["input_tokens", "output_tokens", "tool_call_latency_sec"],
            "distributions": {
                "turns_per_conversation": dist_summary(cr.turns_per_conversation),
                "input_tokens_per_turn": dist_summary(cr.input_tokens_per_turn),
                "output_tokens_per_turn": dist_summary(cr.output_tokens_per_turn),
                "tool_call_latency_sec": dist_summary(cr.tool_call_latency_sec),
            },
            "totals": {
                "total_turns": total_turns,
                "mean_turns_per_conversation": round(float(np.mean(all_turns)), 2),
                "min_turns": int(min(all_turns)),
                "max_turns": int(max(all_turns)),
                "sum_input_tokens": int(sum(all_in)),
                "sum_output_tokens": int(sum(all_out)),
                "total_delay_sec": round(float(sum(all_lat)), 1),
                "mean_delay_sec": round(float(np.mean(all_lat)), 2),
            },
        },
        "conversations": conversations,
    }

    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(trace, f, separators=(",", ":"))

    m = trace["metadata"]["totals"]
    print(f"built {cr.num_conversations} conversations in {build_s:.1f}s")
    print(f"total turns            : {m['total_turns']}")
    print(f"turns/conv mean/min/max: {m['mean_turns_per_conversation']} / {m['min_turns']} / {m['max_turns']}")
    print(f"sum input/output tokens: {m['sum_input_tokens']} / {m['sum_output_tokens']}")
    print(f"mean inter-turn delay  : {m['mean_delay_sec']}s   (total {m['total_delay_sec']}s)")
    print(f"wrote                  : {out_path}")


if __name__ == "__main__":
    main()
