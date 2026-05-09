"""
Example Configuration File for Resilient Request Layer

This file demonstrates how to configure the resilient request layer for different
types of agents with varying performance and cost characteristics.

File: agent-gateway/resilient_config.py
"""

from typing import Dict, Any

# =============================================================================
# AGENT PROFILES
# =============================================================================
# Each agent has different resource requirements and should be rate limited
# and cached appropriately.

AGENT_PROFILES: Dict[str, Dict[str, Any]] = {
    # Fast, low-cost agents that can handle high throughput
    "translator": {
        "name": "A2A Translator",
        "description": "Fast translation service with consistent behavior",
        "category": "utility",
        "requests_per_second": 20.0,      # 20 requests per second sustained
        "burst_capacity": 100,             # Allow spike to 100 requests
        "cache_ttl_seconds": 3600,         # Cache for 1 hour (translations stable)
        "max_queue_size": 200,             # Large queue for this fast agent
        "enable_cache": True,
        "cacheable_probability": 0.8,      # 80% of requests are identical
    },
    
    # Medium-speed, medium-cost agents
    "github_agent": {
        "name": "GitHub Integration Agent",
        "description": "GitHub querying and analysis",
        "category": "integration",
        "requests_per_second": 15.0,
        "burst_capacity": 75,
        "cache_ttl_seconds": 1800,        # Cache for 30 minutes (GitHub data changes)
        "max_queue_size": 150,
        "enable_cache": True,
        "cacheable_probability": 0.6,
    },
    
    # Slow, expensive agents that need strict rate limiting
    "compliance_checker": {
        "name": "A2A Compliance Checker",
        "description": "Deep code compliance analysis",
        "category": "analysis",
        "requests_per_second": 5.0,       # Only 5 sustained requests per second
        "burst_capacity": 25,              # Small burst
        "cache_ttl_seconds": 7200,        # Cache for 2 hours (compliance stable)
        "max_queue_size": 50,              # Smaller queue, prefer queueing to rejection
        "enable_cache": True,
        "cacheable_probability": 0.9,     # Very high cache hit potential
    },
    
    # Real-time agents where caching is less valuable
    "realtime_monitor": {
        "name": "Real-time Monitoring Agent",
        "description": "Live system monitoring and alerts",
        "category": "monitoring",
        "requests_per_second": 10.0,
        "burst_capacity": 50,
        "cache_ttl_seconds": 60,          # Cache only 1 minute (data changes rapidly)
        "max_queue_size": 100,
        "enable_cache": True,
        "cacheable_probability": 0.2,     # Low cache hit rate expected
    },
    
    # Default profile for new/unknown agents
    "default": {
        "name": "Default Agent",
        "description": "Default configuration for unmapped agents",
        "category": "generic",
        "requests_per_second": 10.0,
        "burst_capacity": 50,
        "cache_ttl_seconds": 3600,
        "max_queue_size": 100,
        "enable_cache": True,
        "cacheable_probability": 0.5,
    },
}


# =============================================================================
# GLOBAL SETTINGS
# =============================================================================

GLOBAL_SETTINGS = {
    # Redis Configuration
    "redis": {
        "host": "localhost",           # Redis host
        "port": 6379,                  # Redis port
        "db": 1,                       # Use DB 1 for resilient layer
        "password": None,              # Set if authentication required
        "socket_timeout": 5,           # 5 second connection timeout
        "socket_connect_timeout": 5,
    },
    
    # Cache settings
    "cache": {
        "enabled": True,
        "default_ttl_seconds": 3600,
        "max_entry_size_mb": 10,
        "eviction_policy": "lru",      # lru, lfu, or fifo
        "compression": False,           # Compress large responses (future)
    },
    
    # Rate limiting
    "rate_limiting": {
        "enabled": True,
        "default_rps": 10.0,
        "default_burst": 50,
        "refill_interval_ms": 100,     # Check tokens every 100ms
    },
    
    # Queue settings
    "queuing": {
        "enabled": True,
        "max_queue_size": 100,         # Default per agent
        "queue_timeout_seconds": 3600, # Requests expire after 1 hour in queue
        "processing_batch_size": 10,   # Process 10 at a time
        "processing_interval_ms": 100, # Check queue every 100ms
    },
    
    # Metrics and monitoring
    "metrics": {
        "enabled": True,
        "collection_interval_ms": 1000,    # Aggregate every 1 second
        "retention_seconds": 86400,        # Keep metrics for 24 hours
        "sample_response_times": True,     # Sample (not record) every response time
    },
    
    # Health and diagnostics
    "health": {
        "check_interval_seconds": 60,
        "alert_on_failure": True,
        "failure_threshold_percent": 10,   # Alert if > 10% requests failing
    },
}


# =============================================================================
# SCENARIO: DEVELOPMENT ENVIRONMENT
# =============================================================================

DEVELOPMENT_CONFIG = {
    # Looser limits for testing
    "agent_profiles": {
        agent_id: {
            **profile,
            "requests_per_second": profile["requests_per_second"] * 10,
            "burst_capacity": profile["burst_capacity"] * 10,
            "max_queue_size": profile["max_queue_size"] * 5,
        }
        for agent_id, profile in AGENT_PROFILES.items()
    },
    "global_settings": {
        **GLOBAL_SETTINGS,
        "cache": {
            **GLOBAL_SETTINGS["cache"],
            "default_ttl_seconds": 300,  # Shorter TTL for testing
        },
    },
}


# =============================================================================
# SCENARIO: PRODUCTION ENVIRONMENT
# =============================================================================

PRODUCTION_CONFIG = {
    "agent_profiles": AGENT_PROFILES,
    "global_settings": GLOBAL_SETTINGS,
    # Additional production settings
    "monitoring": {
        "prometheus_enabled": True,
        "prometheus_port": 8001,
        "sentry_enabled": True,
        "sentry_dsn": None,  # Set from env var
        "datadog_enabled": False,
    },
    "alerting": {
        "enabled": True,
        "slack_webhook": None,  # Set from env var
        "alert_thresholds": {
            "cache_hit_ratio_low": 0.3,        # Alert if < 30%
            "queue_full_percent": 0.8,         # Alert if > 80% full
            "error_rate_high": 0.05,           # Alert if > 5% errors
            "response_time_p99_ms": 5000,      # Alert if p99 > 5 seconds
        },
    },
}


# =============================================================================
# SCENARIO: HIGH-TRAFFIC PEAK HOURS
# =============================================================================

PEAK_HOURS_CONFIG = {
    "agent_profiles": {
        agent_id: {
            **profile,
            # Double the RPS and burst during peak hours
            "requests_per_second": profile["requests_per_second"] * 1.5,
            "burst_capacity": profile["burst_capacity"] * 2,
            "max_queue_size": profile["max_queue_size"] * 2,
            # Shorter cache TTL for fresher data during peaks
            "cache_ttl_seconds": int(profile["cache_ttl_seconds"] * 0.5),
        }
        for agent_id, profile in AGENT_PROFILES.items()
    },
    "global_settings": GLOBAL_SETTINGS,
}


# =============================================================================
# SCENARIO: MAINTENANCE MODE (LIMITED SERVICE)
# =============================================================================

MAINTENANCE_CONFIG = {
    "agent_profiles": {
        agent_id: {
            **profile,
            # Reduce by 50%
            "requests_per_second": profile["requests_per_second"] * 0.5,
            "burst_capacity": max(5, profile["burst_capacity"] // 2),
            "max_queue_size": max(10, profile["max_queue_size"] // 2),
        }
        for agent_id, profile in AGENT_PROFILES.items()
    },
    "global_settings": GLOBAL_SETTINGS,
}


# =============================================================================
# UTILITY FUNCTIONS
# =============================================================================

def get_profile(agent_id: str, config_dict: Dict = None) -> Dict[str, Any]:
    """
    Get agent profile from configuration.
    
    Args:
        agent_id: Agent identifier
        config_dict: Configuration dict (defaults to PRODUCTION_CONFIG)
        
    Returns:
        Agent profile including both specific and inherited settings
    """
    if config_dict is None:
        config_dict = PRODUCTION_CONFIG
    
    profiles = config_dict.get("agent_profiles", AGENT_PROFILES)
    profile = profiles.get(agent_id) or profiles.get("default")
    
    # Merge with global defaults
    merged = {
        **profiles.get("default", {}),
        **profile,
    }
    
    return merged


def validate_config(config_dict: Dict) -> bool:
    """Validate configuration dictionary structure."""
    required_keys = ["agent_profiles", "global_settings"]
    
    if not all(key in config_dict for key in required_keys):
        return False
    
    # Validate each agent profile
    for agent_id, profile in config_dict["agent_profiles"].items():
        required_agent_keys = [
            "requests_per_second",
            "burst_capacity",
            "cache_ttl_seconds",
            "max_queue_size",
        ]
        if not all(key in profile for key in required_agent_keys):
            print(f"Invalid agent profile: {agent_id}")
            return False
        
        # Validate values
        if profile["requests_per_second"] <= 0:
            print(f"Invalid RPS for {agent_id}")
            return False
        
        if profile["burst_capacity"] < profile["requests_per_second"]:
            print(f"Burst capacity must be >= RPS for {agent_id}")
            return False
    
    return True


def print_config_summary(config_dict: Dict) -> None:
    """Print a human-readable summary of the configuration."""
    print("\n" + "="*70)
    print("RESILIENT REQUEST LAYER CONFIGURATION")
    print("="*70)
    
    print("\nAgent Profiles:")
    print("-" * 70)
    
    for agent_id, profile in config_dict["agent_profiles"].items():
        print(f"\n  {agent_id} ({profile.get('name', 'Unknown')})")
        print(f"    RPS: {profile['requests_per_second']} sustained, "
              f"{profile['burst_capacity']} burst")
        print(f"    Cache TTL: {profile['cache_ttl_seconds']}s")
        print(f"    Max Queue: {profile['max_queue_size']} requests")
    
    print("\n" + "-" * 70)
    print("Global Settings:")
    global_settings = config_dict["global_settings"]
    print(f"  Cache: {global_settings['cache']['enabled']}")
    print(f"  Rate Limiting: {global_settings['rate_limiting']['enabled']}")
    print(f"  Queuing: {global_settings['queuing']['enabled']}")
    print(f"  Redis DB: {global_settings['redis']['db']}")
    
    print("\n" + "="*70 + "\n")


if __name__ == "__main__":
    # Example usage
    print("Validating production config...")
    if validate_config(PRODUCTION_CONFIG):
        print("✓ Configuration is valid")
    else:
        print("✗ Configuration has errors")
    
    print_config_summary(PRODUCTION_CONFIG)
    
    # Example: get specific agent profile
    profile = get_profile("compliance_checker", PRODUCTION_CONFIG)
    print(f"\nCompliance Checker Profile:")
    print(f"  RPS: {profile['requests_per_second']}")
    print(f"  Burst: {profile['burst_capacity']}")
    print(f"  Cache TTL: {profile['cache_ttl_seconds']}s")
