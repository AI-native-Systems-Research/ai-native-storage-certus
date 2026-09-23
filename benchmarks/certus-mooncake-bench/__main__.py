"""
Entry point for running the benchmark as a module

Usage:
    # Disk baseline (same as Mooncake)
    python -m certus-mooncake-bench --backend disk --scenario toolagent --max-requests=100

    # Certus backend (requires running certus-server + GPU)
    python -m certus-mooncake-bench --backend certus --scenario toolagent --max-requests=100
"""

from benchmark import main

if __name__ == '__main__':
    main()
