# Business Intelligence Agent

A comprehensive Business Intelligence agent that helps organizations analyze business performance, track key metrics, and make data-driven decisions through executive-grade analytics and insights.

## Features

- **Revenue Analytics**: Track revenue trends across different streams and time periods
- **KPI Dashboards**: Department-specific and company-wide performance metrics
- **Business Forecasting**: Predictive analytics for revenue, customer growth, and market expansion
- **Customer Analytics**: Segmentation analysis with churn prediction and retention insights

## Capabilities

### Revenue Analysis
- Multi-stream revenue tracking (Subscriptions, One-time Sales, Professional Services, Partner Revenue)
- Growth rate calculations and trend analysis
- Performance insights and strategic recommendations
- Comparative analysis across time periods

### KPI Monitoring
- Department-specific dashboards (Sales, Marketing, Product, Customer Success, Engineering, HR)
- Performance vs. target tracking
- Industry benchmark comparisons
- Executive summary insights

### Business Forecasting
- Revenue and customer growth projections
- Confidence intervals and scenario planning
- Risk factor assessment
- Strategic planning support

### Customer Segmentation
- Segment performance analysis (Enterprise, SMB, Startups, Individual)
- Churn prediction and retention strategies
- Customer lifetime value insights
- Cross-segment comparative analytics

## Quick Start

### Local Development
```bash
# Set environment variables
export OPENAI_API_KEY="your-openai-api-key"

# Run the agent
python -m src --host localhost --port 5000
```

### Docker
```bash
# Build and run with docker-compose
docker-compose up --build
```

## Example Queries

- "Show me our revenue trends for the last quarter"
- "Generate a KPI dashboard for the sales department with industry benchmarks"
- "Forecast our business metrics for the next 6 months"
- "Analyze our customer segments and churn patterns"
- "What are the key insights from our marketing performance?"

## API Endpoints

The agent exposes standard A2A endpoints:
- `GET /` - Agent card information
- `POST /chat` - Chat with the agent
- `GET /health` - Health check

## Demo Purpose

This agent generates realistic business intelligence data for demonstration purposes. In a production environment, it would integrate with actual business data sources (CRM, ERP, Analytics platforms) to provide real-time insights and analytics.