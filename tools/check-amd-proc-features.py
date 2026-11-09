#!/usr/bin/env python3.12
"""
check_amd_xxh3.py ? AMD CPU feature checker for XXH3 / KV cache integrity
Reports SIMD capabilities, Zen generation, and expected XXH3 throughput tier.
"""

import sys
import subprocess
from dataclasses import dataclass, field
from typing import Optional

try:
    import cpuinfo
except ImportError:
    sys.exit("Missing dependency: pip install py-cpuinfo")


# ?? ANSI colours ????????????????????????????????????????????????????????????

RESET  = "\033[0m"
BOLD   = "\033[1m"
GREEN  = "\033[32m"
YELLOW = "\033[33m"
RED    = "\033[31m"
CYAN   = "\033[36m"
DIM    = "\033[2m"

def ok(s):    return f"{GREEN}?{RESET}  {s}"
def warn(s):  return f"{YELLOW}~{RESET}  {s}"
def miss(s):  return f"{RED}?{RESET}  {s}"
def hdr(s):   return f"\n{BOLD}{CYAN}{s}{RESET}"
def dim(s):   return f"{DIM}{s}{RESET}"


# ?? Zen generation detection ?????????????????????????????????????????????????

@dataclass
class ZenInfo:
    generation:  Optional[int]   # 1-5, or None if unknown
    label:       str              # e.g. "Zen 3"
    example:     str              # representative product
    has_avx512:  bool
    crc32_note:  str


def detect_zen(brand: str, family: int, model: int) -> ZenInfo:
    """
    Map CPUID family/model to Zen generation for EPYC and Ryzen.
    Family 23 = Zen 1/2, Family 25 = Zen 3/4 mobile, Family 26 = Zen 5,
    Family 25 model ?0x10 = Zen 4 EPYC Genoa.
    """
    brand_l = brand.lower()

    # Zen 5 ? family 26
    if family == 26:
        return ZenInfo(5, "Zen 5", "EPYC Turin / Ryzen 9000", True,
                       "CRC32C fast; prefer XXH3 AVX-512")

    # Zen 4 ? family 25, models 0x10-0x1F (EPYC Genoa) or 0x61+ (Ryzen 7000)
    if family == 25 and (0x10 <= model <= 0x1F or model >= 0x61):
        return ZenInfo(4, "Zen 4", "EPYC Genoa / Ryzen 7000", True,
                       "CRC32C fast; AVX-512 at full clock (no throttle)")

    # Zen 3 ? family 25, models 0x00-0x0F (EPYC Milan) or 0x50 (Ryzen 5000)
    if family == 25:
        return ZenInfo(3, "Zen 3", "EPYC Milan / Ryzen 5000", False,
                       "CRC32C latency improved vs Zen 1/2; XXH3 AVX2 recommended")

    # Zen 2 ? family 23, models 0x30+ (EPYC Rome = 0x31) or 0x71 (Ryzen 3000)
    if family == 23 and model >= 0x30:
        return ZenInfo(2, "Zen 2", "EPYC Rome / Ryzen 3000", False,
                       "CRC32C slower than Intel; XXH3 AVX2 is faster choice")

    # Zen 1 ? family 23, lower models
    if family == 23:
        return ZenInfo(1, "Zen 1", "EPYC Naples / Ryzen 1000", False,
                       "CRC32C notably slower than Intel; use XXH3 AVX2")

    return ZenInfo(None, "Unknown AMD", brand, False, "Cannot determine CRC32C quality")


# ?? XXH3 throughput estimates ?????????????????????????????????????????????????

def xxh3_throughput_tier(flags: set, zen: ZenInfo) -> tuple[str, str, str]:
    """
    Returns (tier_name, single_core_estimate, block_latency_260kb).
    Block latency assumes a 260 KB KV cache block (16 tok × 32 layers × 2 × 128d × bf16).
    """
    if "avx512f" in flags and "avx512bw" in flags and zen.has_avx512:
        return ("AVX-512", "~40?50 GB/s", "~5?6 µs")
    elif "avx2" in flags:
        return ("AVX2", "~25?31 GB/s", "~8?10 µs")
    elif "sse2" in flags:
        return ("SSE2", "~12?16 GB/s", "~16?22 µs")
    else:
        return ("Scalar", "~6?8 GB/s", "~33?43 µs")


# ?? Feature groups ????????????????????????????????????????????????????????????

XXH3_REQUIRED = {
    "sse2":   "SSE2 ? minimum for XXH3 vectorised path",
}

XXH3_AVX2 = {
    "avx":    "AVX ? 256-bit foundation",
    "avx2":   "AVX2 ? vpmuludq/vpxor; main XXH3 SIMD path",
    "bmi1":   "BMI1 ? bit manipulation (hash mixing)",
    "bmi2":   "BMI2 ? MULX / RORX used in some hash loops",
}

XXH3_AVX512 = {
    "avx512f":  "AVX-512F ? 512-bit foundation (Zen 4+)",
    "avx512bw": "AVX-512BW ? byte/word ops needed by XXH3 512-bit path",
    "avx512vl": "AVX-512VL ? 128/256-bit evex encoding",
    "avx512dq": "AVX-512DQ ? VPMULUDQ 512-bit variant",
}

STORAGE_INTEGRITY = {
    "sse4_2":   "SSE4.2 ? CRC32 hardware instruction (crc32q)",
    "pclmulqdq":"PCLMUL ? PCLMULQDQ for carry-less multiply (fast CRC32C)",
    "aes":      "AES-NI ? AES acceleration (useful for keyed hashes / BLAKE3 variants)",
    "sha_ni":   "SHA-NI ? hardware SHA-1/SHA-256 (not needed for XXH3)",
}

MEMORY_BANDWIDTH = {
    "clzero":  "CLZERO ? cache line zero (AMD-specific; useful for block eviction)",
    "rdrand":  "RDRAND ? hardware RNG (secret generation for keyed XXH3)",
    "rdseed":  "RDSEED ? higher-entropy RNG",
}


def check_group(label: str, group: dict, flags: set) -> list[str]:
    lines = [hdr(label)]
    for flag, desc in group.items():
        if flag in flags:
            lines.append(ok(f"{BOLD}{flag}{RESET}  {dim(desc)}"))
        else:
            lines.append(miss(f"{BOLD}{flag}{RESET}  {dim(desc)}"))
    return lines


# ?? /proc/cpuinfo fallback for extra AMD flags ????????????????????????????????

def proc_flags() -> set:
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("flags"):
                    return set(line.split(":")[1].split())
    except Exception:
        pass
    return set()


# ?? Main ??????????????????????????????????????????????????????????????????????

def main():
    info   = cpuinfo.get_cpu_info()
    vendor = info.get("vendor_id_raw", "").lower()
    brand  = info.get("brand_raw", "Unknown")
    family = info.get("family", 0)
    model  = info.get("model", 0)
    flags  = set(info.get("flags", []))

    # Merge /proc/cpuinfo flags (py-cpuinfo sometimes misses a few)
    flags |= proc_flags()

    print(f"\n{BOLD}{'?'*60}{RESET}")
    print(f"{BOLD}  AMD XXH3 / KV Cache Integrity Feature Report{RESET}")
    print(f"{BOLD}{'?'*60}{RESET}")

    # ?? CPU identity ??????????????????????????????????????????????????????????
    print(hdr("CPU Identity"))
    print(f"   Brand   : {BOLD}{brand}{RESET}")
    print(f"   Vendor  : {info.get('vendor_id_raw', 'n/a')}")
    print(f"   Arch    : {info.get('arch', 'n/a')}  "
          f"Family {family:#04x}  Model {model:#04x}")
    print(f"   Cores   : {info.get('count', 'n/a')}")
    print(f"   Clock   : {info.get('hz_advertised_friendly', 'n/a')}")

    if "amd" not in vendor and "authenticamd" not in vendor:
        print(f"\n{YELLOW}  ?  Not an AMD CPU ? Zen detection will be skipped.{RESET}")
        print(f"     Vendor reported: {info.get('vendor_id_raw', 'unknown')}\n")
        zen = ZenInfo(None, "N/A (non-AMD)", brand, False, "N/A")
    else:
        zen = detect_zen(brand, family, model)
        print(hdr("Zen Generation"))
        gen_str = f"Zen {zen.generation}" if zen.generation else "Unknown"
        print(f"   Generation : {BOLD}{zen.label}{RESET}")
        print(f"   Example    : {zen.example}")
        print(f"   AVX-512    : {'yes ? full clock, no throttle' if zen.has_avx512 else 'no'}")
        print(f"   CRC32C     : {zen.crc32_note}")

    # ?? Feature groups ????????????????????????????????????????????????????????
    for line in check_group("XXH3 ? Required", XXH3_REQUIRED, flags):
        print(line)
    for line in check_group("XXH3 ? AVX2 Path (Zen 1?5)", XXH3_AVX2, flags):
        print(line)
    for line in check_group("XXH3 ? AVX-512 Path (Zen 4+)", XXH3_AVX512, flags):
        print(line)
    for line in check_group("Storage Integrity (CRC32C / PCLMUL / AES)", STORAGE_INTEGRITY, flags):
        print(line)
    for line in check_group("Memory / RNG Extras", MEMORY_BANDWIDTH, flags):
        print(line)

    # ?? XXH3 verdict ??????????????????????????????????????????????????????????
    tier, throughput, latency = xxh3_throughput_tier(flags, zen)

    print(hdr("XXH3 Throughput Verdict"))
    print(f"   Best path       : {BOLD}{GREEN}{tier}{RESET}")
    print(f"   Single-core est.: {throughput}  (large buffer, DRAM-resident)")
    print(f"   260 KB KV block : {latency} per block")
    print(f"   NVMe latency    : ~100?200 µs  {dim('? checksum overhead is negligible on miss path')}")
    print(f"   DRAM copy time  : ~1.5?2.5 µs  {dim('? checksum adds ~4?6× over raw copy on hit path')}")

    # ?? Algorithm recommendation ???????????????????????????????????????????????
    print(hdr("Certus Algorithm Recommendation"))
    if "avx512f" in flags and zen.has_avx512:
        rec = "XXH3_128bits (AVX-512 path)"
        note = "Full integrity at ~40+ GB/s. No CRC32C needed."
    elif "avx2" in flags:
        rec = "XXH3_128bits (AVX2 path)"
        note = "~25-31 GB/s; 128-bit output avoids birthday collision at scale."
    elif "pclmulqdq" in flags:
        rec = "CRC32C (PCLMULQDQ) or XXH3 SSE2"
        note = "PCLMUL gives ~30 GB/s CRC32C but only 32-bit output ? watch collision budget."
    elif "sse4_2" in flags:
        rec = "CRC32C (crc32q) or XXH3 SSE2"
        note = "crc32q ~12 GB/s on Zen; XXH3 SSE2 comparable, better output width."
    else:
        rec = "XXH3 scalar"
        note = "~6-8 GB/s; still fast enough given NVMe/network dominates miss latency."

    print(f"   Recommended : {BOLD}{GREEN}{rec}{RESET}")
    print(f"   Rationale   : {note}")
    print(f"   Rust crate  : {dim('xxhash-rust = { version = \"0.8\", features = [\"xxh3\"] }')}")
    print(f"                 {dim('twox-hash   (runtime CPUID dispatch ? best for generative components)')}")

    print(f"\n{BOLD}{'?'*60}{RESET}\n")


if __name__ == "__main__":
    main()

