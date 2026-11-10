#!/usr/bin/env python3
"""gen-gqa-iobound-report.py — kept for existing commands; the report is built by
gen-iobound-report.py, which reads the model and workload from each arm's logs.

  ./gen-gqa-iobound-report.py RESULTS_DIR
"""
import os
import runpy
import sys

sys.argv[0] = os.path.join(os.path.dirname(os.path.abspath(__file__)), "gen-iobound-report.py")
runpy.run_path(sys.argv[0], run_name="__main__")
