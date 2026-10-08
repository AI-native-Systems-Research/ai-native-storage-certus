#!/usr/bin/env python3
"""Sweep turns per conversation and report how much of the KV lookup stream SSD serves.

Runs ``kv_cache_dynamics.py`` through ``run-burstgpt-example.sh`` (BurstGPT
think-time intervals + the cc131k model / cache / token configuration) once per
turn count, with every conversation running exactly that many turns, and —
unless ``--no-baseline`` — once more per point with the SSD tier disabled.
Deeper conversations grow a larger KV context (up to the 131K window), so the
working set outgrows HBM + DRAM and spills to SSD.

Same engine and HTML report as ``sweep_dram_ssd.py``; here DRAM is held fixed
(``--dram-gb``, env ``DRAM_GB``, default 32). Any other
``run-cc131k-example.sh`` variable (``CONCURRENT``, ``SSD_GB``,
``PREFIX_TOKENS``, ``ADMISSION``, ...) passes through from the environment.

Examples
--------
    ./sweep_turns_ssd.py                              # writes reports/turns-sweep-report.html
    DRAM_GB=128 CONCURRENT=64 ./sweep_turns_ssd.py -o turns-c64.html
    ./sweep_turns_ssd.py --turn-counts 4,16,64,256,1024
"""

import sys

from sweep_dram_ssd import main

if __name__ == "__main__":
    sys.exit(main("turns", doc=__doc__))
