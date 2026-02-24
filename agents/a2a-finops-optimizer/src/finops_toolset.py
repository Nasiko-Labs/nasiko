import json
from typing import Any, Dict, List
from datetime import datetime, timedelta
import random


class FinOpsToolset:
    """FinOps optimization toolset for cost analysis and recommendations"""
    
    def __init__(self):
        self.cloud_providers = ["AWS", "Azure", "GCP", "DigitalOcean"]
        self.services = {
            "AWS": ["EC2", "RDS", "S3", "Lambda", "ECS", "EKS"],
            "Azure": ["VM", "SQL Database", "Storage", "Functions", "Container Instances", "AKS"],
            "GCP": ["Compute Engine", "Cloud SQL", "Cloud Storage", "Cloud Functions", "GKE"],
            "DigitalOcean": ["Droplets", "Managed Databases", "Spaces", "Functions", "Kubernetes", "App Platform"]
        }
    
    def get_tools(self):
        """Get list of available FinOps tools"""
        return [
            {
                "type": "function",
                "function": {
                    "name": "analyze_cost_trends",
                    "description": "Analyze cost trends across cloud resources for the specified time period",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "time_period": {
                                "type": "string",
                                "description": "Time period for analysis (7d, 30d, 90d)",
                                "enum": ["7d", "30d", "90d"]
                            },
                            "cloud_provider": {
                                "type": "string",
                                "description": "Cloud provider to analyze",
                                "enum": ["AWS", "Azure", "GCP", "DigitalOcean", "all"]
                            }
                        },
                        "required": ["time_period"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "get_optimization_recommendations",
                    "description": "Get cost optimization recommendations based on resource usage patterns",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "service_type": {
                                "type": "string",
                                "description": "Type of service to optimize (compute, storage, database, all)"
                            },
                            "optimization_level": {
                                "type": "string",
                                "description": "Level of optimization aggressiveness",
                                "enum": ["conservative", "moderate", "aggressive"]
                            }
                        },
                        "required": ["service_type"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "forecast_costs",
                    "description": "Forecast future costs based on current usage patterns",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "forecast_period": {
                                "type": "string",
                                "description": "Period to forecast (1m, 3m, 6m, 12m)",
                                "enum": ["1m", "3m", "6m", "12m"]
                            },
                            "include_growth": {
                                "type": "boolean",
                                "description": "Include projected growth in forecast"
                            }
                        },
                        "required": ["forecast_period"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "analyze_resource_utilization",
                    "description": "Analyze resource utilization and identify underutilized resources",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "resource_type": {
                                "type": "string",
                                "description": "Type of resource to analyze"
                            },
                            "utilization_threshold": {
                                "type": "number",
                                "description": "Utilization threshold below which resources are flagged (0-100)"
                            }
                        },
                        "required": ["resource_type"]
                    }
                }
            }
        ]
    
    def analyze_cost_trends(self, time_period: str, cloud_provider: str = "all") -> Dict[str, Any]:
        """Mock cost trends analysis"""
        
        # Normalize time period input
        time_period_map = {
            "7d": 7, "7 days": 7, "one week": 7, "week": 7,
            "30d": 30, "30 days": 30, "one month": 30, "month": 30,
            "90d": 90, "90 days": 90, "three months": 90, "quarter": 90
        }
        
        # Find matching time period
        days = None
        normalized_period = "30d"  # default
        for key, value in time_period_map.items():
            if time_period.lower() == key.lower():
                days = value
                if "7" in key:
                    normalized_period = "7d"
                elif "30" in key:
                    normalized_period = "30d"
                elif "90" in key:
                    normalized_period = "90d"
                break
        
        if days is None:
            days = 30  # default to 30 days
            normalized_period = "30d"
        
        providers = [cloud_provider] if cloud_provider != "all" else self.cloud_providers
        
        cost_data = {}
        total_current_cost = 0
        total_previous_cost = 0
        
        for provider in providers:
            current_cost = random.uniform(5000, 50000)
            previous_cost = current_cost * random.uniform(0.8, 1.2)
            
            cost_data[provider] = {
                "current_period_cost": round(current_cost, 2),
                "previous_period_cost": round(previous_cost, 2),
                "change_percentage": round(((current_cost - previous_cost) / previous_cost) * 100, 2),
                "top_services": []
            }
            
            # Generate top services data
            services = self.services[provider][:3]
            for service in services:
                service_cost = current_cost * random.uniform(0.1, 0.4)
                cost_data[provider]["top_services"].append({
                    "service": service,
                    "cost": round(service_cost, 2),
                    "percentage": round((service_cost / current_cost) * 100, 1)
                })
            
            total_current_cost += current_cost
            total_previous_cost += previous_cost
        
        return {
            "time_period": time_period,
            "analysis_date": datetime.now().isoformat(),
            "total_cost": {
                "current": round(total_current_cost, 2),
                "previous": round(total_previous_cost, 2),
                "change_percentage": round(((total_current_cost - total_previous_cost) / total_previous_cost) * 100, 2)
            },
            "by_provider": cost_data,
            "trends": {
                "highest_growth_provider": max(providers, key=lambda p: cost_data[p]["change_percentage"]),
                "cost_drivers": [
                    "Increased compute usage during peak hours",
                    "New development environment deployments",
                    "Storage costs growing due to data retention"
                ]
            }
        }
    
    def get_optimization_recommendations(self, service_type: str, optimization_level: str = "moderate") -> Dict[str, Any]:
        """Generate mock optimization recommendations"""
        
        recommendations = []
        potential_savings = 0
        
        if service_type in ["compute", "all"]:
            compute_savings = random.uniform(2000, 8000)
            potential_savings += compute_savings
            recommendations.append({
                "category": "Compute Optimization",
                "priority": "High",
                "savings_potential": round(compute_savings, 2),
                "recommendations": [
                    {
                        "action": "Right-size compute instances",
                        "description": "14 instances (Droplets/EC2/VMs) are over-provisioned with average CPU utilization below 20%",
                        "estimated_monthly_savings": round(compute_savings * 0.6, 2),
                        "effort": "Low",
                        "risk": "Low"
                    },
                    {
                        "action": "Implement auto-scaling",
                        "description": "Configure auto-scaling groups for variable workloads across cloud providers",
                        "estimated_monthly_savings": round(compute_savings * 0.4, 2),
                        "effort": "Medium",
                        "risk": "Low"
                    }
                ]
            })
        
        if service_type in ["storage", "all"]:
            storage_savings = random.uniform(1000, 3000)
            potential_savings += storage_savings
            recommendations.append({
                "category": "Storage Optimization",
                "priority": "Medium",
                "savings_potential": round(storage_savings, 2),
                "recommendations": [
                    {
                        "action": "Implement intelligent tiering",
                        "description": "Move infrequently accessed data to cheaper storage tiers",
                        "estimated_monthly_savings": round(storage_savings * 0.7, 2),
                        "effort": "Low",
                        "risk": "Low"
                    },
                    {
                        "action": "Clean up old snapshots",
                        "description": "Remove snapshots older than 90 days",
                        "estimated_monthly_savings": round(storage_savings * 0.3, 2),
                        "effort": "Low",
                        "risk": "Medium"
                    }
                ]
            })
        
        if service_type in ["database", "all"]:
            db_savings = random.uniform(1500, 5000)
            potential_savings += db_savings
            recommendations.append({
                "category": "Database Optimization",
                "priority": "High",
                "savings_potential": round(db_savings, 2),
                "recommendations": [
                    {
                        "action": "Optimize RDS instance types",
                        "description": "Switch to newer generation instances with better price-performance ratio",
                        "estimated_monthly_savings": round(db_savings * 0.6, 2),
                        "effort": "Medium",
                        "risk": "Medium"
                    },
                    {
                        "action": "Enable automated backups optimization",
                        "description": "Reduce backup retention and optimize backup frequency",
                        "estimated_monthly_savings": round(db_savings * 0.4, 2),
                        "effort": "Low",
                        "risk": "Low"
                    }
                ]
            })
        
        return {
            "analysis_date": datetime.now().isoformat(),
            "optimization_level": optimization_level,
            "total_potential_savings": round(potential_savings, 2),
            "annual_savings_projection": round(potential_savings * 12, 2),
            "recommendations": recommendations,
            "implementation_timeline": {
                "quick_wins": "2-4 weeks",
                "medium_effort": "1-2 months",
                "complex_changes": "2-3 months"
            }
        }
    
    def forecast_costs(self, forecast_period: str, include_growth: bool = True) -> Dict[str, Any]:
        """Generate cost forecast"""
        
        current_monthly_cost = random.uniform(15000, 45000)
        
        # Normalize forecast period input
        period_map = {
            "1m": 1, "1 month": 1, "one month": 1, "month": 1,
            "3m": 3, "3 months": 3, "three months": 3, "quarter": 3,
            "6m": 6, "6 months": 6, "six months": 6, "half year": 6,
            "12m": 12, "12 months": 12, "one year": 12, "year": 12, "annual": 12
        }
        
        # Find matching period
        months = None
        normalized_period = "6m"  # default
        for key, value in period_map.items():
            if forecast_period.lower() == key.lower():
                months = value
                if value == 1:
                    normalized_period = "1m"
                elif value == 3:
                    normalized_period = "3m" 
                elif value == 6:
                    normalized_period = "6m"
                elif value == 12:
                    normalized_period = "12m"
                break
        
        if months is None:
            months = 6  # default to 6 months
            normalized_period = "6m"
        
        # Base growth rate
        monthly_growth_rate = random.uniform(0.02, 0.08) if include_growth else 0
        
        forecasted_costs = []
        for month in range(1, months + 1):
            cost = current_monthly_cost * (1 + monthly_growth_rate) ** month
            forecasted_costs.append({
                "month": month,
                "forecasted_cost": round(cost, 2)
            })
        
        total_forecasted = sum(month["forecasted_cost"] for month in forecasted_costs)
        
        return {
            "forecast_date": datetime.now().isoformat(),
            "forecast_period": forecast_period,
            "current_monthly_cost": round(current_monthly_cost, 2),
            "growth_rate_included": include_growth,
            "monthly_growth_rate": round(monthly_growth_rate * 100, 2) if include_growth else 0,
            "forecasted_costs": forecasted_costs,
            "total_forecasted_cost": round(total_forecasted, 2),
            "confidence_level": "85%",
            "assumptions": [
                "Current usage patterns remain consistent",
                "No major architectural changes",
                "Market pricing remains stable",
                "Seasonal variations accounted for"
            ]
        }
    
    def analyze_resource_utilization(self, resource_type: str, utilization_threshold: float = 30.0) -> Dict[str, Any]:
        """Analyze resource utilization patterns"""
        
        underutilized_resources = []
        
        # Generate mock underutilized resources
        for i in range(random.randint(3, 12)):
            utilization = random.uniform(5, utilization_threshold)
            monthly_cost = random.uniform(100, 2000)
            
            underutilized_resources.append({
                "resource_id": f"{resource_type}-{random.randint(1000, 9999)}",
                "resource_name": f"prod-{resource_type}-{i+1}",
                "current_utilization": round(utilization, 1),
                "monthly_cost": round(monthly_cost, 2),
                "recommended_action": "downsize" if utilization < 15 else "optimize",
                "potential_monthly_savings": round(monthly_cost * (1 - utilization/100), 2)
            })
        
        total_waste = sum(r["potential_monthly_savings"] for r in underutilized_resources)
        
        return {
            "analysis_date": datetime.now().isoformat(),
            "resource_type": resource_type,
            "utilization_threshold": utilization_threshold,
            "total_resources_analyzed": random.randint(50, 200),
            "underutilized_resources_count": len(underutilized_resources),
            "underutilized_resources": underutilized_resources,
            "total_monthly_waste": round(total_waste, 2),
            "annual_waste_projection": round(total_waste * 12, 2),
            "utilization_summary": {
                "average_utilization": round(random.uniform(45, 75), 1),
                "resources_below_threshold": len(underutilized_resources),
                "optimization_opportunity": "High" if len(underutilized_resources) > 8 else "Medium"
            }
        }