"""Weather monitoring crew using CrewAI framework."""

import os
from crewai import Agent, Task, Crew, Process
from crewai.tools import BaseTool
from weather_toolset import WeatherToolset
from typing import Dict, Any
from pydantic import Field
import json


class WeatherAnalysisTool(BaseTool):
    """Tool for weather analysis using CrewAI."""
    name: str = "weather_analysis"
    description: str = "Analyze weather data and provide insights"
    weather_toolset: WeatherToolset = Field(default_factory=WeatherToolset)
    
    def _run(self, location: str, analysis_type: str = "current") -> str:
        """Run weather analysis."""
        try:
            if analysis_type == "current":
                result = self.weather_toolset.get_current_weather_sync(location)
            elif analysis_type == "forecast":
                result = self.weather_toolset.get_weather_forecast_sync(location)
            elif analysis_type == "alerts":
                result = self.weather_toolset.get_weather_alerts_sync(location)
            else:
                result = {"error": f"Unknown analysis type: {analysis_type}"}
            
            return json.dumps(result)
        except Exception as e:
            return json.dumps({"error": str(e)})


class WeatherForecastTool(BaseTool):
    """Tool for weather forecasting using CrewAI."""
    name: str = "weather_forecast"
    description: str = "Get detailed weather forecasts"
    weather_toolset: WeatherToolset = Field(default_factory=WeatherToolset)
    
    def _run(self, location: str, days: int = 5) -> str:
        """Get weather forecast."""
        try:
            result = self.weather_toolset.get_weather_forecast_sync(location, days)
            return json.dumps(result)
        except Exception as e:
            return json.dumps({"error": str(e)})


class WeatherAlertsTool(BaseTool):
    """Tool for weather alerts using CrewAI."""
    name: str = "weather_alerts"
    description: str = "Monitor weather alerts and warnings"
    weather_toolset: WeatherToolset = Field(default_factory=WeatherToolset)
    
    def _run(self, location: str) -> str:
        """Get weather alerts."""
        try:
            result = self.weather_toolset.get_weather_alerts_sync(location)
            return json.dumps(result)
        except Exception as e:
            return json.dumps({"error": str(e)})


def create_weather_crew():
    """Create a CrewAI crew for weather monitoring."""
    
    # Initialize tools
    weather_analysis_tool = WeatherAnalysisTool()
    weather_forecast_tool = WeatherForecastTool()
    weather_alerts_tool = WeatherAlertsTool()
    
    # Create weather monitoring agent
    weather_analyst = Agent(
        role='Weather Analyst',
        goal='Provide accurate and comprehensive weather information and analysis',
        backstory="""You are an expert meteorologist with extensive experience in weather 
        analysis, forecasting, and risk assessment. You specialize in providing clear, 
        actionable weather insights for various purposes including travel planning, 
        outdoor activities, and emergency preparedness.""",
        verbose=True,
        allow_delegation=False,
        tools=[weather_analysis_tool, weather_forecast_tool, weather_alerts_tool]
    )
    
    # Create weather monitoring specialist
    weather_monitor = Agent(
        role='Weather Monitor',
        goal='Monitor weather conditions and provide timely alerts and warnings',
        backstory="""You are a weather monitoring specialist focused on real-time weather 
        tracking and alert systems. You excel at identifying potentially dangerous weather 
        conditions and communicating them effectively to ensure public safety.""",
        verbose=True,
        allow_delegation=False,
        tools=[weather_analysis_tool, weather_alerts_tool]
    )
    
    # Create forecast specialist
    forecast_specialist = Agent(
        role='Forecast Specialist',
        goal='Provide detailed and accurate weather forecasts',
        backstory="""You are a forecasting expert who specializes in predicting weather 
        patterns and trends. You have a deep understanding of meteorological models and 
        can translate complex weather data into understandable forecasts for the general public.""",
        verbose=True,
        allow_delegation=False,
        tools=[weather_forecast_tool, weather_analysis_tool]
    )
    
    return Crew(
        agents=[weather_analyst, weather_monitor, forecast_specialist],
        tasks=[],  # Tasks will be created dynamically based on requests
        process=Process.sequential,
        verbose=True
    )


def create_weather_task(crew: Crew, query: str, location: str = None) -> Dict[str, Any]:
    """Create and execute a weather task based on the query."""
    
    # Determine the primary agent and task based on the query
    if "current" in query.lower() or "now" in query.lower():
        agent = crew.agents[0]  # weather_analyst
        task_description = f"Get current weather conditions for {location or 'the specified location'}"
        expected_output = "Current weather conditions including temperature, humidity, wind speed, and general conditions"
    elif "forecast" in query.lower() or "tomorrow" in query.lower() or "week" in query.lower():
        agent = crew.agents[2]  # forecast_specialist
        task_description = f"Provide weather forecast for {location or 'the specified location'}"
        expected_output = "Detailed weather forecast with daily conditions, temperatures, and precipitation chances"
    elif "alert" in query.lower() or "warning" in query.lower():
        agent = crew.agents[1]  # weather_monitor
        task_description = f"Check for weather alerts and warnings for {location or 'the specified location'}"
        expected_output = "Weather alerts and warnings with severity levels and recommended actions"
    else:
        agent = crew.agents[0]  # default to weather_analyst
        task_description = f"Analyze weather information for {location or 'the specified location'} based on the query: {query}"
        expected_output = "Comprehensive weather analysis addressing the specific query"
    
    # Create the task
    task = Task(
        description=task_description,
        expected_output=expected_output,
        agent=agent
    )
    
    # Update crew tasks and execute
    crew.tasks = [task]
    result = crew.kickoff()
    
    return {
        "query": query,
        "location": location,
        "agent_role": agent.role,
        "result": str(result)
    }