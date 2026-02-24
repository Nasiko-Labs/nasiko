"""Data Analysis Agent implementation using LangChain."""

from data_analysis_toolset import DataAnalysisToolset
from langchain_core.tools import BaseTool
from typing import Dict, Any, List, Union
from pydantic import BaseModel, Field
import json


class AnalyzeDatasetTool(BaseTool):
    """Tool for analyzing datasets."""
    name: str = "analyze_dataset"
    description: str = "Analyze a dataset and return basic statistics and information"
    toolset: DataAnalysisToolset = Field(default_factory=DataAnalysisToolset)
    
    def _run(self, data: str, file_type: str = "csv") -> str:
        result = self.toolset.analyze_dataset(data, file_type)
        return json.dumps(result)
        
    async def _arun(self, data: str, file_type: str = "csv") -> str:
        return self._run(data, file_type)

class CreateVisualizationTool(BaseTool):
    """Tool for creating visualizations."""
    name: str = "create_visualization"
    description: str = "Create a chart or graph from the data"
    toolset: DataAnalysisToolset = Field(default_factory=DataAnalysisToolset)
    
    def _run(self, data: str, chart_type: str, x_column: str, y_column: str = None) -> str:
        result = self.toolset.create_visualization(data, chart_type, x_column, y_column)
        return json.dumps(result)
        
    async def _arun(self, data: str, chart_type: str, x_column: str, y_column: str = None) -> str:
        return self._run(data, chart_type, x_column, y_column)

class CalculateMetricsTool(BaseTool):
    """Tool for calculating metrics."""
    name: str = "calculate_metrics"
    description: str = "Calculate various metrics and KPIs from the data"
    toolset: DataAnalysisToolset = Field(default_factory=DataAnalysisToolset)
    
    def _run(self, data: str, metric_type: str, column: str = None) -> str:
        result = self.toolset.calculate_metrics(data, metric_type, column)
        return json.dumps(result)
        
    async def _arun(self, data: str, metric_type: str, column: str = None) -> str:
        return self._run(data, metric_type, column)

def create_agent() -> Dict[str, Any]:
    """Create the data analysis agent with tools and system prompt."""
    
    toolset = DataAnalysisToolset()
    
    # Create LangChain tools
    langchain_tools = [
        AnalyzeDatasetTool(),
        CreateVisualizationTool(), 
        CalculateMetricsTool()
    ]
    
    # OpenAI-compatible tools for A2A framework
    tools = [
        {
            "type": "function",
            "function": {
                "name": "analyze_dataset",
                "description": "Analyze a dataset and return basic statistics and information",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "data": {
                            "type": "string",
                            "description": "The CSV data as a string"
                        },
                        "file_type": {
                            "type": "string",
                            "description": "The type of file (default: csv)",
                            "default": "csv"
                        }
                    },
                    "required": ["data"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_visualization",
                "description": "Create a chart or graph from the data",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "data": {
                            "type": "string",
                            "description": "The CSV data as a string"
                        },
                        "chart_type": {
                            "type": "string",
                            "description": "Type of chart to create (histogram, scatter, bar, line)",
                            "enum": ["histogram", "scatter", "bar", "line"]
                        },
                        "x_column": {
                            "type": "string",
                            "description": "The column name for x-axis"
                        },
                        "y_column": {
                            "type": "string",
                            "description": "The column name for y-axis (optional for some chart types)"
                        }
                    },
                    "required": ["data", "chart_type", "x_column"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "calculate_metrics",
                "description": "Calculate various metrics and KPIs from the data",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "data": {
                            "type": "string",
                            "description": "The CSV data as a string"
                        },
                        "metric_type": {
                            "type": "string",
                            "description": "Type of metric to calculate",
                            "enum": ["correlation", "growth_rate", "summary"]
                        },
                        "column": {
                            "type": "string",
                            "description": "Column name for specific metrics (required for growth_rate and summary)"
                        }
                    },
                    "required": ["data", "metric_type"]
                }
            }
        }
    ]

    system_prompt = """You are a Data Analysis Agent specialized in analyzing datasets and creating visualizations.

Your capabilities include:
1. Analyzing datasets to provide descriptive statistics, data types, and missing value information
2. Creating various types of visualizations (histograms, scatter plots, bar charts, line charts)
3. Calculating key metrics like correlation matrices, growth rates, and summary statistics

When analyzing data:
- Always start by examining the structure and basic statistics of the dataset
- Look for missing values, data types, and potential data quality issues
- Provide clear, actionable insights from your analysis
- Suggest appropriate visualizations based on the data types and user request

When creating visualizations:
- Choose appropriate chart types based on the data and analysis goals
- Ensure x and y columns are valid for the chosen chart type
- Provide meaningful titles and descriptions for charts

When calculating metrics:
- Explain what each metric means in the context of the data
- Provide interpretation of results where appropriate
- Suggest follow-up analyses when relevant

Always format your responses clearly and provide actionable insights from the data analysis."""

    # Create a mapping for tool calls (direct callable methods)
    tool_mapping = {
        "analyze_dataset": toolset.analyze_dataset,
        "create_visualization": toolset.create_visualization,
        "calculate_metrics": toolset.calculate_metrics,
    }

    return {
        "tools": tools,
        "tool_mapping": tool_mapping,
        "langchain_tools": langchain_tools,
        "system_prompt": system_prompt,
    }