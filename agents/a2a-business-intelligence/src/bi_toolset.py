import json
from typing import Any, Dict, List
from datetime import datetime, timedelta
import random


class BusinessIntelligenceToolset:
    """Business Intelligence toolset for analytics and business metrics"""
    
    def __init__(self):
        self.departments = ["Sales", "Marketing", "Product", "Customer Success", "Engineering", "HR"]
        self.metrics_categories = ["Revenue", "Customer", "Product", "Operational", "Financial"]
        self.revenue_streams = ["Subscriptions", "One-time Sales", "Professional Services", "Partner Revenue"]
        self.customer_segments = ["Enterprise", "SMB", "Startups", "Individual"]
    
    def get_tools(self):
        """Get list of available BI tools"""
        return [
            {
                "type": "function",
                "function": {
                    "name": "analyze_revenue_trends",
                    "description": "Analyze revenue trends and performance metrics for specified time period",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "time_period": {
                                "type": "string",
                                "description": "Time period for analysis (1m, 3m, 6m, 12m)",
                                "enum": ["1m", "3m", "6m", "12m"]
                            },
                            "revenue_stream": {
                                "type": "string",
                                "description": "Revenue stream to analyze",
                                "enum": ["Subscriptions", "One-time Sales", "Professional Services", "Partner Revenue", "all"]
                            }
                        },
                        "required": ["time_period"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "get_kpi_dashboard",
                    "description": "Get key performance indicators dashboard with current metrics and targets",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "department": {
                                "type": "string",
                                "description": "Department to focus on",
                                "enum": ["Sales", "Marketing", "Product", "Customer Success", "Engineering", "HR", "all"]
                            },
                            "include_benchmarks": {
                                "type": "boolean",
                                "description": "Include industry benchmarks"
                            }
                        },
                        "required": ["department"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "forecast_business_metrics",
                    "description": "Forecast key business metrics based on historical data and trends",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "forecast_period": {
                                "type": "string",
                                "description": "Period to forecast (3m, 6m, 12m)",
                                "enum": ["3m", "6m", "12m"]
                            },
                            "metric_type": {
                                "type": "string",
                                "description": "Type of metric to forecast",
                                "enum": ["revenue", "customers", "growth", "all"]
                            }
                        },
                        "required": ["forecast_period"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "analyze_customer_segments",
                    "description": "Analyze customer segment performance and behavior patterns",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "segment": {
                                "type": "string",
                                "description": "Customer segment to analyze",
                                "enum": ["Enterprise", "SMB", "Startups", "Individual", "all"]
                            },
                            "include_churn_analysis": {
                                "type": "boolean",
                                "description": "Include churn prediction analysis"
                            }
                        },
                        "required": ["segment"]
                    }
                }
            }
        ]
    
    def analyze_revenue_trends(self, time_period: str, revenue_stream: str = "all") -> Dict[str, Any]:
        """Analyze revenue trends and performance"""
        
        # Normalize time period input
        period_map = {
            "1m": 1, "1 month": 1, "one month": 1, "month": 1,
            "3m": 3, "3 months": 3, "three months": 3, "quarter": 3,
            "6m": 6, "6 months": 6, "six months": 6, "half year": 6,
            "12m": 12, "12 months": 12, "one year": 12, "year": 12, "annual": 12
        }
        
        # Find matching period
        months = period_map.get(time_period.lower(), 3)
        
        # Generate mock revenue data
        streams = [revenue_stream] if revenue_stream != "all" else self.revenue_streams
        
        revenue_data = {}
        total_current_revenue = 0
        total_previous_revenue = 0
        
        for stream in streams:
            # Generate realistic revenue numbers based on stream type
            if stream == "Subscriptions":
                current_revenue = random.uniform(500000, 2000000)
            elif stream == "One-time Sales":
                current_revenue = random.uniform(100000, 800000)
            elif stream == "Professional Services":
                current_revenue = random.uniform(50000, 300000)
            else:  # Partner Revenue
                current_revenue = random.uniform(25000, 150000)
                
            previous_revenue = current_revenue * random.uniform(0.7, 1.3)
            growth_rate = ((current_revenue - previous_revenue) / previous_revenue) * 100
            
            revenue_data[stream] = {
                "current_revenue": round(current_revenue, 2),
                "previous_revenue": round(previous_revenue, 2),
                "growth_rate": round(growth_rate, 2),
                "monthly_trend": [
                    round(current_revenue * random.uniform(0.8, 1.2), 2) 
                    for _ in range(months)
                ]
            }
            
            total_current_revenue += current_revenue
            total_previous_revenue += previous_revenue
        
        overall_growth = ((total_current_revenue - total_previous_revenue) / total_previous_revenue) * 100
        
        return {
            "analysis_date": datetime.now().isoformat(),
            "time_period": time_period,
            "total_revenue": {
                "current": round(total_current_revenue, 2),
                "previous": round(total_previous_revenue, 2),
                "growth_rate": round(overall_growth, 2)
            },
            "by_stream": revenue_data,
            "key_insights": [
                "Subscription revenue shows strong recurring growth momentum",
                "Professional services demand indicates market expansion opportunity",
                "Partner channel performance suggests need for enablement focus"
            ],
            "recommendations": [
                "Focus on subscription customer retention to maintain MRR growth",
                "Expand professional services team to capture consulting demand",
                "Implement partner enablement program to boost channel performance"
            ]
        }
    
    def get_kpi_dashboard(self, department: str, include_benchmarks: bool = True) -> Dict[str, Any]:
        """Generate KPI dashboard for specified department"""
        
        departments = [department] if department != "all" else self.departments
        
        kpi_data = {}
        
        for dept in departments:
            if dept == "Sales":
                kpis = {
                    "monthly_recurring_revenue": {
                        "current": round(random.uniform(800000, 1500000), 2),
                        "target": round(random.uniform(1200000, 1800000), 2),
                        "previous": round(random.uniform(700000, 1400000), 2),
                        "unit": "USD"
                    },
                    "new_customers_acquired": {
                        "current": random.randint(45, 120),
                        "target": random.randint(80, 150),
                        "previous": random.randint(40, 110),
                        "unit": "count"
                    },
                    "sales_cycle_length": {
                        "current": random.randint(25, 45),
                        "target": random.randint(20, 35),
                        "previous": random.randint(30, 50),
                        "unit": "days"
                    }
                }
            elif dept == "Marketing":
                kpis = {
                    "cost_per_acquisition": {
                        "current": round(random.uniform(150, 400), 2),
                        "target": round(random.uniform(100, 300), 2),
                        "previous": round(random.uniform(200, 450), 2),
                        "unit": "USD"
                    },
                    "marketing_qualified_leads": {
                        "current": random.randint(200, 500),
                        "target": random.randint(300, 600),
                        "previous": random.randint(180, 450),
                        "unit": "count"
                    },
                    "website_conversion_rate": {
                        "current": round(random.uniform(2.5, 8.0), 2),
                        "target": round(random.uniform(5.0, 10.0), 2),
                        "previous": round(random.uniform(2.0, 7.5), 2),
                        "unit": "percentage"
                    }
                }
            elif dept == "Product":
                kpis = {
                    "monthly_active_users": {
                        "current": random.randint(15000, 50000),
                        "target": random.randint(25000, 75000),
                        "previous": random.randint(12000, 45000),
                        "unit": "count"
                    },
                    "feature_adoption_rate": {
                        "current": round(random.uniform(35, 75), 2),
                        "target": round(random.uniform(60, 85), 2),
                        "previous": round(random.uniform(30, 70), 2),
                        "unit": "percentage"
                    },
                    "user_engagement_score": {
                        "current": round(random.uniform(6.5, 9.2), 2),
                        "target": round(random.uniform(8.0, 9.5), 2),
                        "previous": round(random.uniform(6.0, 8.8), 2),
                        "unit": "score"
                    }
                }
            elif dept == "Customer Success":
                kpis = {
                    "customer_satisfaction_score": {
                        "current": round(random.uniform(7.5, 9.5), 2),
                        "target": round(random.uniform(8.5, 9.8), 2),
                        "previous": round(random.uniform(7.0, 9.2), 2),
                        "unit": "score"
                    },
                    "net_promoter_score": {
                        "current": random.randint(40, 70),
                        "target": random.randint(60, 80),
                        "previous": random.randint(35, 65),
                        "unit": "score"
                    },
                    "churn_rate": {
                        "current": round(random.uniform(2.0, 8.0), 2),
                        "target": round(random.uniform(1.0, 5.0), 2),
                        "previous": round(random.uniform(3.0, 9.0), 2),
                        "unit": "percentage"
                    }
                }
            else:  # Generic department KPIs
                kpis = {
                    "productivity_index": {
                        "current": round(random.uniform(75, 95), 2),
                        "target": round(random.uniform(85, 100), 2),
                        "previous": round(random.uniform(70, 90), 2),
                        "unit": "score"
                    },
                    "efficiency_ratio": {
                        "current": round(random.uniform(1.2, 2.1), 2),
                        "target": round(random.uniform(1.5, 2.5), 2),
                        "previous": round(random.uniform(1.1, 2.0), 2),
                        "unit": "ratio"
                    }
                }
            
            # Calculate performance indicators
            for kpi_name, kpi_data_item in kpis.items():
                achievement_rate = (kpi_data_item["current"] / kpi_data_item["target"]) * 100
                trend = "up" if kpi_data_item["current"] > kpi_data_item["previous"] else "down"
                kpi_data_item["achievement_rate"] = round(achievement_rate, 2)
                kpi_data_item["trend"] = trend
            
            kpi_data[dept] = {
                "kpis": kpis,
                "overall_performance": round(random.uniform(75, 95), 2)
            }
        
        return {
            "dashboard_date": datetime.now().isoformat(),
            "department": department,
            "kpi_data": kpi_data,
            "executive_summary": {
                "top_performing_metric": "Monthly Recurring Revenue showing 23% growth",
                "attention_needed": "Customer acquisition cost trending above target",
                "overall_health": "Strong" if random.choice([True, False]) else "Good"
            },
            "industry_benchmarks": {
                "customer_satisfaction": "8.2 (Industry Average)",
                "churn_rate": "5.5% (Industry Average)",
                "sales_cycle": "35 days (Industry Average)"
            } if include_benchmarks else None
        }
    
    def forecast_business_metrics(self, forecast_period: str, metric_type: str = "all") -> Dict[str, Any]:
        """Forecast business metrics based on historical trends"""
        
        # Normalize forecast period
        period_map = {"3m": 3, "6m": 6, "12m": 12}
        months = period_map.get(forecast_period.lower(), 6)
        
        current_monthly_revenue = random.uniform(800000, 1500000)
        current_customers = random.randint(5000, 15000)
        
        # Generate growth rates
        revenue_growth_rate = random.uniform(0.03, 0.08)  # 3-8% monthly
        customer_growth_rate = random.uniform(0.02, 0.06)  # 2-6% monthly
        
        forecasts = {}
        
        if metric_type in ["revenue", "all"]:
            revenue_forecast = []
            for month in range(1, months + 1):
                forecasted_revenue = current_monthly_revenue * (1 + revenue_growth_rate) ** month
                revenue_forecast.append({
                    "month": month,
                    "forecasted_revenue": round(forecasted_revenue, 2),
                    "confidence_interval": {
                        "lower": round(forecasted_revenue * 0.85, 2),
                        "upper": round(forecasted_revenue * 1.15, 2)
                    }
                })
            
            forecasts["revenue"] = {
                "forecast_data": revenue_forecast,
                "total_forecasted": round(sum(f["forecasted_revenue"] for f in revenue_forecast), 2),
                "growth_assumptions": f"{revenue_growth_rate*100:.1f}% monthly growth"
            }
        
        if metric_type in ["customers", "all"]:
            customer_forecast = []
            for month in range(1, months + 1):
                forecasted_customers = int(current_customers * (1 + customer_growth_rate) ** month)
                customer_forecast.append({
                    "month": month,
                    "forecasted_customers": forecasted_customers,
                    "new_acquisitions": int(forecasted_customers * customer_growth_rate),
                    "churn_estimate": int(forecasted_customers * random.uniform(0.02, 0.05))
                })
            
            forecasts["customers"] = {
                "forecast_data": customer_forecast,
                "total_forecasted": customer_forecast[-1]["forecasted_customers"],
                "growth_assumptions": f"{customer_growth_rate*100:.1f}% monthly customer growth"
            }
        
        if metric_type in ["growth", "all"]:
            growth_metrics = {
                "market_expansion": round(random.uniform(15, 35), 2),
                "product_adoption": round(random.uniform(20, 45), 2),
                "operational_efficiency": round(random.uniform(10, 25), 2)
            }
            forecasts["growth"] = growth_metrics
        
        return {
            "forecast_date": datetime.now().isoformat(),
            "forecast_period": forecast_period,
            "metric_type": metric_type,
            "forecasts": forecasts,
            "confidence_level": "85%",
            "key_assumptions": [
                "Market conditions remain stable",
                "No major competitive disruptions",
                "Current growth trends continue",
                "Operational capacity scales appropriately"
            ],
            "risk_factors": [
                "Economic downturn impacting customer spending",
                "Increased competition affecting market share",
                "Operational scaling challenges"
            ]
        }
    
    def analyze_customer_segments(self, segment: str, include_churn_analysis: bool = True) -> Dict[str, Any]:
        """Analyze customer segment performance and behavior"""
        
        segments = [segment] if segment != "all" else self.customer_segments
        
        segment_data = {}
        
        for seg in segments:
            # Generate segment-specific metrics
            if seg == "Enterprise":
                customer_count = random.randint(50, 200)
                avg_revenue = random.uniform(50000, 150000)
                satisfaction = random.uniform(8.0, 9.5)
                churn_rate = random.uniform(1.0, 3.0)
            elif seg == "SMB":
                customer_count = random.randint(500, 2000)
                avg_revenue = random.uniform(5000, 25000)
                satisfaction = random.uniform(7.5, 9.0)
                churn_rate = random.uniform(3.0, 7.0)
            elif seg == "Startups":
                customer_count = random.randint(800, 3000)
                avg_revenue = random.uniform(1000, 8000)
                satisfaction = random.uniform(7.0, 8.5)
                churn_rate = random.uniform(5.0, 12.0)
            else:  # Individual
                customer_count = random.randint(2000, 10000)
                avg_revenue = random.uniform(100, 2000)
                satisfaction = random.uniform(6.5, 8.0)
                churn_rate = random.uniform(8.0, 15.0)
            
            total_revenue = customer_count * avg_revenue
            
            segment_analysis = {
                "customer_count": customer_count,
                "total_revenue": round(total_revenue, 2),
                "average_revenue_per_customer": round(avg_revenue, 2),
                "customer_satisfaction_score": round(satisfaction, 2),
                "market_share": round(random.uniform(15, 35), 2),
                "growth_rate": round(random.uniform(-5, 25), 2)
            }
            
            if include_churn_analysis:
                segment_analysis["churn_analysis"] = {
                    "current_churn_rate": round(churn_rate, 2),
                    "predicted_churn": round(churn_rate * 1.1, 2),
                    "at_risk_customers": int(customer_count * churn_rate / 100),
                    "retention_recommendations": [
                        "Implement targeted engagement campaigns",
                        "Enhance customer success touchpoints",
                        "Develop segment-specific product features"
                    ]
                }
            
            segment_data[seg] = segment_analysis
        
        return {
            "analysis_date": datetime.now().isoformat(),
            "segment": segment,
            "segment_data": segment_data,
            "cross_segment_insights": {
                "highest_value_segment": max(segment_data.keys(), 
                    key=lambda s: segment_data[s]["average_revenue_per_customer"]),
                "fastest_growing_segment": max(segment_data.keys(), 
                    key=lambda s: segment_data[s]["growth_rate"]),
                "most_satisfied_segment": max(segment_data.keys(), 
                    key=lambda s: segment_data[s]["customer_satisfaction_score"])
            },
            "strategic_recommendations": [
                "Focus retention efforts on high-value Enterprise segment",
                "Invest in product features for growing SMB market",
                "Develop specialized onboarding for Startup segment"
            ]
        }