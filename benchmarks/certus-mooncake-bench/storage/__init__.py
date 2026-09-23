"""
KVCache Storage Module

Provides storage backend implementations for KVCache systems.
"""

from .interface import Storage
from .disk import DiskHashTable

# CertusStorage is imported lazily (requires CUDA + running certus-server)
def get_certus_storage_class():
    from .certus import CertusStorage
    return CertusStorage

__all__ = [
    'Storage',
    'DiskHashTable',
    'get_certus_storage_class',
]
