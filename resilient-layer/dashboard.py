"""
AgentShield Real-Time Monitoring Dashboard
Streamlit-based operational visibility for buildathon demo.
"""
import streamlit as st
import requests
import time
import plotly.graph_objects as go
from plotly.subplots import make_subplots
from datetime import datetime
import pandas as pd

# Configuration
METRICS_URL = "http://localhost:8500/metrics"
REFRESH_INTERVAL = 2  # seconds

st.set_page_config(
    page_title="AgentShield Dashboard",
    page_icon="🛡️",
    layout="wide"
)

st.title("🛡️ AgentShield - Resilient Agent Request Layer")
st.markdown("*Real-time monitoring for multi-agent traffic control*")

# Auto-refresh
st.caption(f"Auto-refreshing every {REFRESH_INTERVAL}s | Last update: {datetime.now().strftime('%H:%M:%S')}")

# Create placeholders for metrics
metrics_placeholder = st.empty()

def fetch_metrics():
    """Fetch metrics from resilient layer"""
    try:
        response = requests.get(METRICS_URL, timeout=2)
        if response.status_code == 200:
            return response.json()
        return None
    except:
        return None

def create_cache_chart(cache_data):
    """Create cache hit/miss chart"""
    if not cache_data:
        return None
    
    labels = ['Cache Hits', 'Cache Misses', 'Semantic Hits']
    values = [
        cache_data.get('hits', 0) - cache_data.get('semantic_hits', 0),
        cache_data.get('misses', 0),
        cache_data.get('semantic_hits', 0)
    ]
    
    fig = go.Figure(data=[go.Pie(labels=labels, values=values, hole=.4)])
    fig.update_layout(title="Cache Distribution", height=300)
    return fig

def create_rate_limit_chart(rate_data):
    """Create rate limit utilization chart"""
    if not rate_data:
        return None
    
    agents = list(rate_data.keys())
    utilization = [rate_data[a]['utilization_percent'] for a in agents]
    
    fig = go.Figure(data=[
        go.Bar(name='Utilization %', x=agents, y=utilization,
               marker_color=['green' if u < 70 else 'orange' if u < 90 else 'red' for u in utilization])
    ])
    fig.update_layout(title="Rate Limit Utilization by Agent", height=300,
                     yaxis_title="Utilization %")
    return fig

def main():
    """Main dashboard loop"""
    
    while True:
        metrics = fetch_metrics()
        
        with metrics_placeholder.container():
            if metrics:
                # Header metrics
                req_data = metrics.get('requests', {})
                cache_data = metrics.get('cache', {})
                queue_data = metrics.get('queue', {})
                coalesce_data = metrics.get('coalescing', {})
                
                # KPI Row
                col1, col2, col3, col4, col5 = st.columns(5)
                
                with col1:
                    st.metric(
                        "Total Requests",
                        req_data.get('total_requests', 0),
                        f"{req_data.get('requests_per_second', 0)}/s"
                    )
                
                with col2:
                    hit_rate = cache_data.get('hit_rate_percent', 0)
                    st.metric(
                        "Cache Hit Rate",
                        f"{hit_rate:.1f}%",
                        delta=f"{cache_data.get('semantic_hits', 0)} semantic"
                    )
                
                with col3:
                    saved = coalesce_data.get('saved_computations', 0)
                    st.metric(
                        "Computations Saved",
                        saved,
                        delta=f"{coalesce_data.get('coalesced_requests', 0)} coalesced"
                    )
                
                with col4:
                    queue_size = queue_data.get('current_queue_size', 0)
                    st.metric(
                        "Queue Depth",
                        queue_size,
                        delta=f"{queue_data.get('success_rate', 0)}% success"
                    )
                
                with col5:
                    latency = req_data.get('latency', {})
                    avg_lat = latency.get('average_ms', 0)
                    st.metric(
                        "Avg Latency",
                        f"{avg_lat:.1f}ms",
                        delta=f"min: {latency.get('min_ms', 0):.1f}ms"
                    )
                
                # Charts row
                col_left, col_right = st.columns(2)
                
                with col_left:
                    cache_chart = create_cache_chart(cache_data)
                    if cache_chart:
                        st.plotly_chart(cache_chart, use_container_width=True)
                
                with col_right:
                    rate_chart = create_rate_limit_chart(metrics.get('rate_limits', {}))
                    if rate_chart:
                        st.plotly_chart(rate_chart, use_container_width=True)
                
                # Detailed stats expander
                with st.expander("Detailed Metrics"):
                    st.json(metrics)
            else:
                st.warning("⚠️ Unable to connect to resilient layer. Make sure it's running on port 8500.")
        
        time.sleep(REFRESH_INTERVAL)

if __name__ == "__main__":
    main()
