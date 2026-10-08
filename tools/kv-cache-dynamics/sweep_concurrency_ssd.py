#!/usr/bin/env python3
"""Sweep concurrent sessions and report how much of the KV lookup stream SSD serves.

Runs ``kv_cache_dynamics.py`` through ``run-burstgpt-example.sh`` (BurstGPT
think-time intervals + the cc131k model / cache / token configuration) once per
concurrency level — the number of conversations in flight at once, held
constant — and, unless ``--no-baseline``, once more per point with the SSD tier
disabled. More concurrent conversations means more foreign traffic during each
think-time pause, so a returning session finds its KV further down the tiers.

Same engine and HTML report as ``sweep_dram_ssd.py``; here DRAM (``--dram-gb``,
env ``DRAM_GB``, default 32) and turns (``--num-turns``, env ``NUM_TURNS``,
default 100) are held fixed. ``--sessions`` defaults to 512: the first
concurrency wave is warmup and excluded from the stats, so the total must exceed
the largest level. Any other ``run-cc131k-example.sh`` variable (``SSD_GB``,
``PREFIX_TOKENS``, ``ADMISSION``, ...) passes through from the environment.

Examples
--------
    ./sweep_concurrency_ssd.py                        # writes reports/concurrency-sweep-report.html
    DRAM_GB=256 ./sweep_concurrency_ssd.py -o conc-dram256.html
    ./sweep_concurrency_ssd.py --concurrency 1,4,16,64,256 --sessions 1024
"""

import sys

from sweep_dram_ssd import main

if __name__ == "__main__":
    sys.exit(main("concurrency", doc=__doc__))
