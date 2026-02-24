import os
from typing import List, Optional

class ObservabilityConfig:
    """Centralized configuration for observability features"""
    
    @staticmethod
    def get_phoenix_endpoint() -> str:
        """Get Phoenix collector endpoint from environment"""
        return os.getenv(
            "PHOENIX_COLLECTOR_ENDPOINT",
            "http://phoenix-service.nasiko.svc.cluster.local:6006/v1/traces"
        )
    
    @staticmethod
    def is_tracing_enabled() -> bool:
        """Check if tracing is enabled"""
        return os.getenv("TRACING_ENABLED", "true").lower() == "true"
    
    @staticmethod
    def get_project_prefix() -> str:
        """Get project name prefix"""
        return os.getenv("TRACING_PROJECT_PREFIX", "")
    
    @staticmethod
    def get_required_dependencies() -> List[str]:
        """Get list of required tracing dependencies"""
        return [
            "arize-phoenix>=12.0.0",
            "openinference-instrumentation-openai>=0.1.40",
            "openinference-instrumentation-langchain>=0.1.57", 
            "opentelemetry-api>=1.36.0",
            "opentelemetry-sdk>=1.36.0",
            "opentelemetry-exporter-otlp>=1.36.0",
            "pytz"
        ]
    
    @staticmethod
    def get_injection_enabled() -> bool:
        """Check if automatic injection is enabled"""
        return os.getenv("OBSERVABILITY_INJECTION_ENABLED", "true").lower() == "true"
    
    @staticmethod
    def get_log_level() -> str:
        """Get observability log level"""
        return os.getenv("OBSERVABILITY_LOG_LEVEL", "INFO")