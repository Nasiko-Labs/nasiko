# FinOps Optimizer Agent

A comprehensive FinOps (Financial Operations) agent that helps organizations analyze, optimize, and forecast their cloud infrastructure costs across multiple cloud providers.

## Features

- **Cost Trend Analysis**: Analyze cost patterns across AWS, Azure, and GCP over different time periods
- **Optimization Recommendations**: Get actionable recommendations for cost reduction with estimated savings
- **Cost Forecasting**: Predict future cloud costs based on usage patterns and growth projections
- **Resource Utilization Analysis**: Identify underutilized resources and waste opportunities

## Capabilities

### Cost Analysis
- Multi-cloud cost tracking (AWS, Azure, GCP)
- Historical trend analysis (7d, 30d, 90d periods)
- Service-level cost breakdown
- Growth rate calculations

### Optimization Recommendations
- Compute optimization (right-sizing, auto-scaling)
- Storage optimization (tiering, cleanup)
- Database optimization (instance types, backup policies)
- Risk and effort assessments for each recommendation

### Forecasting
- Monthly and annual cost projections
- Growth rate modeling
- Confidence intervals and assumptions
- Scenario-based planning

### Resource Utilization
- Utilization threshold analysis
- Waste identification and quantification
- Resource-specific optimization suggestions
- Cost impact assessments

## Quick Start

### Local Development
```bash
# Set environment variables
export OPENAI_API_KEY="your-openai-api-key"

# Run the agent
python -m src --host localhost --port 10008
```

### Docker
```bash
# Build and run with docker-compose
docker-compose up --build
```

## Example Queries

- "Analyze our cloud cost trends for the last 30 days"
- "What are the top cost optimization recommendations for our compute resources?"
- "Forecast our cloud costs for the next 6 months with expected growth"
- "Show me underutilized resources with less than 30% utilization"
- "Give me aggressive optimization recommendations for all services"

## API Endpoints

The agent exposes standard A2A endpoints:
- `GET /` - Agent card information
- `POST /chat` - Chat with the agent
- `GET /health` - Health check

## Demo Purpose

This agent generates realistic mock data for demonstration purposes. In a production environment, it would integrate with actual cloud provider APIs (AWS Cost Explorer, Azure Cost Management, GCP Billing API) to fetch real cost and utilization data.