"""
KVCache Layout — MLA and Generic model configurations

Implements the KVLayout interface for:
- MLA architecture (GLM-5, Kimi-K2.6): per-layer decomposition
- Generic models (Llama, Mistral, Qwen, DeepSeek, etc.): bytes_per_token based

Model KV cache sizes from https://kvcache.ai/tools/kv-cache-calculator/
"""

from typing import Iterator, Any

from .interface import KVLayout, StorageAccess


# ============================================================================
# Model Configurations
# ============================================================================

# MLA Model configurations (require per-layer decomposition)
# Source: https://kvcache.ai/tools/kv-cache-calculator/
MLA_MODEL_CONFIG = {
    # GLM-5: 78 layers, 64 tokens/page
    # KV: 78 layers × 64 tokens × (512+64+128) × 2 = 90,112 bytes/page
    # Per token: 1,408 bytes
    "glm5": {
        "name": "glm5",
        "num_layers": 78,
        "kv_lora_rank": 512,
        "qk_rope_head_dim": 64,
        "index_head_dim": 128,
        "kv_precision_bytes": 2,  # BF16
        "indexer_precision_bytes": 2,  # BF16
    },

    # Kimi-K2.6: 61 layers, 64 tokens/page
    # KV: 61 layers × 64 tokens × (512+64) × 2 = 73,728 bytes/page
    # Per token: 1,152 bytes
    "kimi-k2.6": {
        "name": "kimi-k2.6",
        "num_layers": 61,
        "kv_lora_rank": 512,
        "qk_rope_head_dim": 64,
        "index_head_dim": 0,  # Kimi doesn't use separate indexer
        "kv_precision_bytes": 2,  # BF16
        "indexer_precision_bytes": 0,
    },
}

# Generic model configurations (bytes_per_token based)
# Source: https://kvcache.ai/tools/kv-cache-calculator/
#
# KV cache bytes per token for common model architectures:
#
# ┌─────────────────────────┬──────────────┬──────────────────────────┐
# │ Model                   │ Bytes/Token  │ Notes                    │
# ├─────────────────────────┼──────────────┼──────────────────────────┤
# │ Small Models (7B-13B)   │              │                          │
# │  llama-3-8b             │          128 │ GQA optimized            │
# │  mistral-7b             │          128 │ GQA optimized            │
# │  qwen-14b               │           40 │ GQA optimized            │
# │  gemma-7b               │          224 │                          │
# │  llama-2-7b             │          512 │                          │
# │  llama-2-13b            │          800 │                          │
# ├─────────────────────────┼──────────────┼──────────────────────────┤
# │ Large Models (70B-405B) │              │                          │
# │  llama-2-70b            │          320 │ GQA optimized            │
# │  llama-3-70b            │          320 │ GQA optimized            │
# │  mixtral-8x7b           │          128 │ GQA optimized            │
# │  mixtral-8x22b          │          224 │ GQA optimized            │
# │  qwen-72b              │          320 │ GQA optimized            │
# │  qwen-110b              │          320 │ GQA optimized            │
# │  llama-3.1-405b         │       516018 │ ~504 KB/token            │
# ├─────────────────────────┼──────────────┼──────────────────────────┤
# │ Extra Large             │              │                          │
# │  glm-4.6                │       156991 │ ~153 KB/token            │
# │  deepseek-v3            │      1749384 │ ~1.67 MB/token (largest) │
# ├─────────────────────────┼──────────────┼──────────────────────────┤
# │  default                │         2048 │ Legacy 7B FP16           │
# └─────────────────────────┴──────────────┴──────────────────────────┘
GENERIC_MODEL_CONFIG = {
    # Small models (7B-13B)
    "llama-3-8b":       {"name": "llama-3-8b",       "bytes_per_token": 128,     "notes": "GQA optimized"},
    "mistral-7b":       {"name": "mistral-7b",       "bytes_per_token": 128,     "notes": "GQA optimized"},
    "qwen-14b":         {"name": "qwen-14b",         "bytes_per_token": 40,      "notes": "GQA optimized"},
    "gemma-7b":         {"name": "gemma-7b",         "bytes_per_token": 224,     "notes": ""},
    "llama-2-7b":       {"name": "llama-2-7b",       "bytes_per_token": 512,     "notes": ""},
    "llama-2-13b":      {"name": "llama-2-13b",      "bytes_per_token": 800,     "notes": ""},

    # Large models (70B-405B)
    "llama-2-70b":      {"name": "llama-2-70b",      "bytes_per_token": 320,     "notes": "GQA optimized"},
    "llama-3-70b":      {"name": "llama-3-70b",      "bytes_per_token": 320,     "notes": "GQA optimized"},
    "mixtral-8x7b":     {"name": "mixtral-8x7b",     "bytes_per_token": 128,     "notes": "GQA optimized"},
    "mixtral-8x22b":    {"name": "mixtral-8x22b",    "bytes_per_token": 224,     "notes": "GQA optimized"},
    "qwen-72b":         {"name": "qwen-72b",         "bytes_per_token": 320,     "notes": "GQA optimized"},
    "qwen-110b":        {"name": "qwen-110b",        "bytes_per_token": 320,     "notes": "GQA optimized"},
    "llama-3.1-405b":   {"name": "llama-3.1-405b",   "bytes_per_token": 516018,  "notes": "~504 KB/token"},

    # Extra large
    "glm-4.6":          {"name": "glm-4.6",          "bytes_per_token": 156991,  "notes": "~153 KB/token"},
    "deepseek-v3":      {"name": "deepseek-v3",      "bytes_per_token": 1749384, "notes": "~1.67 MB/token (largest)"},

    # Default (legacy 7B FP16)
    "default":          {"name": "default",           "bytes_per_token": 2048,    "notes": "Legacy 7B FP16"},
}


class MLALayout(KVLayout):
    """MLA (Multi-head Latent Attention) KVCache layout

    MLA Architecture:
    - Each hash_id corresponds to a 512-token chunk
    - Each hash_id maps to ONE complete entry containing all layers
    - Entry contains KV + Indexer data for all layers for 512 tokens
    - Value size is fixed per hash_id (includes all layers)

    Value Size Calculation (per hash_id entry):
        per_layer_size = 512 × (kv_lora_rank + qk_rope_head_dim + index_head_dim) × precision_bytes
        value_size = per_layer_size × num_layers

    For GLM-5 with 512 tokens per entry:
        per_layer_size = 512 × (512 + 64 + 128) × 2 = 720,896 bytes
        value_size = 720,896 × 78 = 56,229,888 bytes = 53.6 MiB

    Key Pattern: hash_id → single entry (all layers included)
    Total Keys = len(hash_ids)

    Used in: GLM-5, Kimi-K2.6
    """

    def __init__(self, num_layers: int, kv_lora_rank: int, qk_rope_head_dim: int,
                 index_head_dim: int, precision_bytes: int, page_size_tokens: int = 512):
        """Initialize MLA layout

        Args:
            num_layers: Number of transformer layers
            kv_lora_rank: KV LoRA rank dimension
            qk_rope_head_dim: QK rope head dimension
            index_head_dim: Indexer head dimension
            precision_bytes: Precision in bytes (BF16=2, INT8=1, INT4=0.5)
            page_size_tokens: Tokens per page (default: 512)
        """
        self.num_layers = num_layers
        self.kv_lora_rank = kv_lora_rank
        self.qk_rope_head_dim = qk_rope_head_dim
        self.index_head_dim = index_head_dim
        self.precision_bytes = precision_bytes
        self.page_size_tokens = page_size_tokens

        # Calculate fixed value size per entry (per hash_id)
        # Each entry contains KV + Indexer for all layers for page_size_tokens
        # Per layer: page_size_tokens × (kv_lora_rank + qk_rope_head_dim + index_head_dim) × precision_bytes
        # Total: per_layer_size × num_layers
        per_layer_size = page_size_tokens * (kv_lora_rank + qk_rope_head_dim + index_head_dim) * precision_bytes
        self.value_size_bytes = per_layer_size * num_layers

        # Store page_size for backward compatibility
        self.page_size = self.value_size_bytes

    def get_operations(self, request: Any) -> Iterator[StorageAccess]:
        """Generate storage access requirements for a request

        For MLA architecture:
        - Each hash_id corresponds to one complete page (512 tokens, all layers)
        - Generate one access requirement per hash_id

        Args:
            request: KVCache request with hash_ids, input_length, output_length

        Yields:
            StorageAccess: Page access requirements
        """
        for hash_id in request.hash_ids:
            yield StorageAccess(
                page_id=hash_id,
                offset_in_page=0,
                length=self.value_size_bytes
            )


class GenericLayout(KVLayout):
    """Generic KVCache layout for non-MLA models

    Uses a flat bytes_per_token value. Page size = bytes_per_token × tokens_per_page.
    Each hash_id maps to one page, same as MLA.
    """

    def __init__(self, bytes_per_token: int, page_size_tokens: int = 512):
        self.bytes_per_token = bytes_per_token
        self.page_size_tokens = page_size_tokens
        self.value_size_bytes = bytes_per_token * page_size_tokens
        self.page_size = self.value_size_bytes

    def get_operations(self, request: Any) -> Iterator[StorageAccess]:
        for hash_id in request.hash_ids:
            yield StorageAccess(
                page_id=hash_id,
                offset_in_page=0,
                length=self.value_size_bytes
            )


# ============================================================================
# Utility Functions
# ============================================================================

# All available models (MLA + generic)
ALL_MODELS = {}
ALL_MODELS.update(MLA_MODEL_CONFIG)
ALL_MODELS.update(GENERIC_MODEL_CONFIG)


def get_model_config(model_name: str) -> dict:
    """Get model configuration by name

    Supports MLA models (glm5, kimi-k2.6) and generic models (llama-3-8b,
    deepseek-v3, etc.). Use --model=<name> or --bytes-per-token=N for custom.

    Args:
        model_name: Model identifier (e.g., 'glm5', 'llama-3-70b', 'deepseek-v3')

    Returns:
        dict: Model configuration

    Raises:
        KeyError: If model name is not found
    """
    if model_name in ALL_MODELS:
        return ALL_MODELS[model_name].copy()

    available_mla = ", ".join(MLA_MODEL_CONFIG.keys())
    available_generic = ", ".join(GENERIC_MODEL_CONFIG.keys())
    raise KeyError(
        f"Unknown model: {model_name}.\n"
        f"  MLA models: {available_mla}\n"
        f"  Generic models: {available_generic}"
    )


def create_layout(model_config: dict, page_size_tokens: int = 64) -> KVLayout:
    """Create a layout instance from model configuration

    Dispatches to MLALayout for MLA models (have 'num_layers') or GenericLayout
    for generic models (have 'bytes_per_token').

    Args:
        model_config: Model configuration dictionary
        page_size_tokens: Tokens per page (default 64)

    Returns:
        KVLayout: Layout instance (MLALayout or GenericLayout)
    """
    if 'bytes_per_token' in model_config:
        # Generic model
        return GenericLayout(
            bytes_per_token=model_config['bytes_per_token'],
            page_size_tokens=page_size_tokens,
        )

    # MLA model
    required_fields = ['num_layers', 'kv_lora_rank', 'qk_rope_head_dim',
                       'index_head_dim', 'kv_precision_bytes']
    for field in required_fields:
        if field not in model_config:
            raise ValueError(f"Missing required field: {field}")

    return MLALayout(
        num_layers=model_config['num_layers'],
        kv_lora_rank=model_config['kv_lora_rank'],
        qk_rope_head_dim=model_config['qk_rope_head_dim'],
        index_head_dim=model_config['index_head_dim'],
        precision_bytes=model_config['kv_precision_bytes'],
        page_size_tokens=page_size_tokens,
    )
