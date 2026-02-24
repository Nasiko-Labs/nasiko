"""Data analysis toolset for the A2A Data Analysis Agent."""

import pandas as pd
import numpy as np
import matplotlib.pyplot as plt
import seaborn as sns
import plotly.express as px
import plotly.graph_objects as go
from typing import Dict, Any, List
import io
import base64
import json


class DataAnalysisToolset:
    """Tools for data analysis and visualization."""

    def analyze_dataset(self, data: str, file_type: str = "csv") -> Dict[str, Any]:
        """Analyze a dataset and return basic statistics."""
        try:
            # Parse the data
            if file_type.lower() == "csv":
                df = pd.read_csv(io.StringIO(data))
            else:
                return {"error": "Unsupported file type. Only CSV is supported."}

            # Basic info
            analysis = {
                "shape": df.shape,
                "columns": df.columns.tolist(),
                "data_types": df.dtypes.astype(str).to_dict(),
                "missing_values": df.isnull().sum().to_dict(),
                "summary_statistics": {}
            }

            # Summary statistics for numeric columns
            numeric_cols = df.select_dtypes(include=[np.number]).columns
            if len(numeric_cols) > 0:
                analysis["summary_statistics"] = df[numeric_cols].describe().to_dict()

            # Categorical columns info
            categorical_cols = df.select_dtypes(include=['object']).columns
            if len(categorical_cols) > 0:
                analysis["categorical_info"] = {}
                for col in categorical_cols:
                    analysis["categorical_info"][col] = {
                        "unique_count": df[col].nunique(),
                        "top_values": df[col].value_counts().head(5).to_dict()
                    }

            return analysis

        except Exception as e:
            return {"error": f"Error analyzing dataset: {str(e)}"}

    def create_visualization(self, data: str, chart_type: str, x_column: str, y_column: str = None) -> Dict[str, Any]:
        """Create a visualization from the data."""
        try:
            df = pd.read_csv(io.StringIO(data))

            if x_column not in df.columns:
                return {"error": f"Column '{x_column}' not found in dataset"}

            if y_column and y_column not in df.columns:
                return {"error": f"Column '{y_column}' not found in dataset"}

            # Create visualization based on chart type
            if chart_type.lower() == "histogram":
                fig = px.histogram(df, x=x_column, title=f"Histogram of {x_column}")
            elif chart_type.lower() == "scatter":
                if not y_column:
                    return {"error": "Scatter plot requires both x and y columns"}
                fig = px.scatter(df, x=x_column, y=y_column, title=f"{x_column} vs {y_column}")
            elif chart_type.lower() == "bar":
                if y_column:
                    fig = px.bar(df, x=x_column, y=y_column, title=f"{y_column} by {x_column}")
                else:
                    # Count plot
                    value_counts = df[x_column].value_counts()
                    fig = px.bar(x=value_counts.index, y=value_counts.values, 
                               title=f"Count of {x_column}")
            elif chart_type.lower() == "line":
                if not y_column:
                    return {"error": "Line plot requires both x and y columns"}
                fig = px.line(df, x=x_column, y=y_column, title=f"{y_column} over {x_column}")
            else:
                return {"error": f"Unsupported chart type: {chart_type}"}

            # Convert to image
            img_buffer = io.BytesIO()
            fig.write_image(img_buffer, format='png')
            img_buffer.seek(0)
            img_base64 = base64.b64encode(img_buffer.getvalue()).decode()

            return {
                "chart_type": chart_type,
                "image": img_base64,
                "description": f"{chart_type.title()} chart of {x_column}" + (f" vs {y_column}" if y_column else "")
            }

        except Exception as e:
            return {"error": f"Error creating visualization: {str(e)}"}

    def calculate_metrics(self, data: str, metric_type: str, column: str = None) -> Dict[str, Any]:
        """Calculate various metrics from the data."""
        try:
            df = pd.read_csv(io.StringIO(data))

            if metric_type.lower() == "correlation":
                # Calculate correlation matrix for numeric columns
                numeric_cols = df.select_dtypes(include=[np.number]).columns
                if len(numeric_cols) < 2:
                    return {"error": "Need at least 2 numeric columns for correlation"}
                
                corr_matrix = df[numeric_cols].corr()
                return {
                    "metric_type": "correlation",
                    "correlation_matrix": corr_matrix.to_dict()
                }

            elif metric_type.lower() == "growth_rate" and column:
                if column not in df.columns:
                    return {"error": f"Column '{column}' not found"}
                
                if not pd.api.types.is_numeric_dtype(df[column]):
                    return {"error": f"Column '{column}' is not numeric"}

                # Calculate period-over-period growth rate
                growth_rates = df[column].pct_change() * 100
                
                return {
                    "metric_type": "growth_rate",
                    "column": column,
                    "average_growth_rate": growth_rates.mean(),
                    "growth_rates": growth_rates.dropna().tolist()
                }

            elif metric_type.lower() == "summary" and column:
                if column not in df.columns:
                    return {"error": f"Column '{column}' not found"}

                if pd.api.types.is_numeric_dtype(df[column]):
                    stats = df[column].describe()
                    return {
                        "metric_type": "summary",
                        "column": column,
                        "statistics": stats.to_dict()
                    }
                else:
                    # Categorical summary
                    value_counts = df[column].value_counts()
                    return {
                        "metric_type": "summary",
                        "column": column,
                        "unique_values": df[column].nunique(),
                        "most_common": value_counts.head(5).to_dict()
                    }

            else:
                return {"error": f"Unsupported metric type: {metric_type}"}

        except Exception as e:
            return {"error": f"Error calculating metrics: {str(e)}"}