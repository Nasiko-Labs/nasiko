"""
Observability utilities for automatic tracing injection
"""

# Runtime imports - always available
from .tracing_utils import bootstrap_tracing

# Build-time imports - only available during injection
try:
    from .config import ObservabilityConfig
    from .injector import TracingInjector
    __all__ = ["bootstrap_tracing", "ObservabilityConfig", "TracingInjector"]
except ImportError:
    # At runtime in agent containers, only tracing_utils is needed
    __all__ = ["bootstrap_tracing"]