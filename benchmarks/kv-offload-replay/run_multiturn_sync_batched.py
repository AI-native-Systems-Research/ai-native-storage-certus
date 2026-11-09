"""run_multiturn_sync_batched.py — the synchronous batched-round execution model.

This is the default, synchronous way the five backend drivers replay the
multi-turn ShareGPT workload: round ``k`` submits, for every still-alive
conversation, the cumulative prompt in one ``llm.generate`` batch, then folds
each response back in for the next round. It is the counterpart to the async
per-conversation model in :mod:`run_multiturn_async`.

The shared, execution-model-agnostic pieces — dataset loading
(``load_convs``), engine construction (``build_engine``), the tokenizer-length
closure (``make_n_tokens``), the MFU probe, the Prometheus exporter, and the
telemetry helpers — live in :mod:`run_multiturn_common`. A driver builds its
backend engine there, then hands this loop the ``LLM``, its ``SamplingParams``,
an ``n_tokens`` callable, and optional per-round callbacks.

Behavior is intentionally identical to the pre-extraction inline loops. The two
places the drivers differed are parameters here:

* ``skip_empty`` — nooffload and the shmq driver drop a turn whose prompt
  tokenizes to 0 tokens (granite renders some ShareGPT turns empty, which aborts
  the vLLM engine); the offload/SharedStorage drivers did not have that guard.
* ``session_id_fn`` — the shmq driver tags every request with a per-conversation
  KV-offload ``session_id`` via a cloned ``SamplingParams``; the others pass one
  shared ``SamplingParams``.

Nothing here imports vllm, so the driver stays in control of when the (heavy)
engine import happens.
"""

import time


def run_batched(llm, convs, sampling_params, *, prompt_budget, max_rounds,
                n_tokens, skip_empty=False, session_id_fn=None,
                on_round_start=None, on_round_end=None):
    """Drive the multi-turn workload as synchronous batched rounds.

    Round k submits, for every still-alive conversation, the cumulative prompt
    (all prior human turns + all prior vLLM responses + the k'th human turn) in
    one ``llm.generate`` batch, then appends each response for the next round.

    Parameters
    ----------
    llm : vllm.LLM
        Constructed by the driver with its backend ``kv_transfer_config``.
    convs : list[list[str]]
        Human-turn streams from :func:`run_multiturn_common.load_convs`.
    sampling_params : vllm.SamplingParams
        Base sampling params. When ``session_id_fn`` is given it is
        ``.clone()``d per request; otherwise the same object is reused.
    prompt_budget : int
        Max prompt tokens (``max_model_len - output_tokens``); a conversation
        whose next prompt exceeds this is dropped from further rounds.
    max_rounds : int
        Cap on rounds (0 = run until conversations are exhausted).
    n_tokens : Callable[[str], int]
        Token-length function (the driver owns tokenizer choice).
    skip_empty : bool
        Also drop a turn whose prompt tokenizes to 0 tokens.
    session_id_fn : Callable[[int], int] | None
        If given, each request gets ``sampling_params.clone()`` with
        ``extra_args={"kv_transfer_params": {"session_id": session_id_fn(i)}}``
        where ``i`` is the conversation index.
    on_round_start : Callable[[int, int], None] | None
        Called after the round's batch is assembled, before ``generate``, as
        ``on_round_start(round_idx, n_prompts)``. Use to snapshot pre-generate
        counters (disk bytes, etc.).
    on_round_end : Callable[[int, int, float, int], None] | None
        Called after outputs are folded back in, as
        ``on_round_end(round_idx, n_prompts, round_elapsed, n_alive)``. Use to
        snapshot post-generate counters, print the ``[run] round N:`` line, and
        record per-round telemetry.

    Returns
    -------
    dict with ``rounds_done``, ``total_generations`` and ``elapsed`` (loop-only
    wall time, excluding model load).
    """
    n = len(convs)
    alive = [True] * n
    next_turn = [0] * n
    contexts = [""] * n

    rounds_done = 0
    total_generations = 0
    t_start = time.perf_counter()

    while True:
        if max_rounds and rounds_done >= max_rounds:
            break

        active_idx = []
        active_prompts = []
        active_sps = [] if session_id_fn is not None else None
        for i, conv in enumerate(convs):
            if not alive[i]:
                continue
            k = next_turn[i]
            if k >= len(conv):
                alive[i] = False
                continue
            human = conv[k]
            candidate = human if k == 0 else contexts[i] + "\n\n" + human
            nt = n_tokens(candidate)
            if (skip_empty and nt == 0) or nt > prompt_budget:
                alive[i] = False
                continue
            contexts[i] = candidate
            active_idx.append(i)
            active_prompts.append(candidate)
            if session_id_fn is not None:
                # Tag each request with its conversation as the KV-offload
                # session_id. The conversation index is stable across rounds, so
                # every turn of the same conversation shares one session_id.
                sp_i = sampling_params.clone()
                sp_i.extra_args = {
                    "kv_transfer_params": {"session_id": session_id_fn(i)}
                }
                active_sps.append(sp_i)

        if not active_prompts:
            break

        rounds_done += 1
        if on_round_start is not None:
            on_round_start(rounds_done, len(active_prompts))

        round_start = time.perf_counter()
        outs = llm.generate(
            active_prompts,
            active_sps if session_id_fn is not None else sampling_params,
        )
        round_elapsed = time.perf_counter() - round_start

        for i, out in zip(active_idx, outs):
            response = out.outputs[0].text if out.outputs else ""
            contexts[i] = contexts[i] + response
            next_turn[i] += 1
        total_generations += len(active_prompts)
        n_alive = sum(alive)

        if on_round_end is not None:
            on_round_end(rounds_done, len(active_prompts), round_elapsed, n_alive)

    elapsed = time.perf_counter() - t_start
    return {
        "rounds_done": rounds_done,
        "total_generations": total_generations,
        "elapsed": elapsed,
    }
