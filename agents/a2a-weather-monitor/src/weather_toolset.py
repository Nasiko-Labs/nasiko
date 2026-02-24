"""Weather monitoring toolset for the A2A Weather Monitor Agent."""

import aiohttp
import asyncio
import ssl
from typing import Dict, Any, Optional
import json


class WeatherToolset:
    """Tools for weather monitoring and forecasting."""
    
    def __init__(self):
        self.base_url = "http://api.openweathermap.org/data/2.5"
        # For demo purposes, using a free API that doesn't require a key
        # In production, you would use a proper API key
        self.fallback_url = "https://api.open-meteo.com/v1"
        # Create SSL context that doesn't verify certificates (for demo purposes)
        # In production, you should use proper certificates
        self.ssl_context = ssl.create_default_context()
        self.ssl_context.check_hostname = False
        self.ssl_context.verify_mode = ssl.CERT_NONE

    async def get_current_weather(self, city: str, country: str = "US") -> Dict[str, Any]:
        """Get current weather conditions for a specified location."""
        try:
            # Using Open-Meteo API as it's free and doesn't require API key
            # First, get coordinates for the city
            geocoding_url = f"https://geocoding-api.open-meteo.com/v1/search?name={city}&count=1"
            
            async with aiohttp.ClientSession(connector=aiohttp.TCPConnector(ssl=self.ssl_context)) as session:
                async with session.get(geocoding_url) as response:
                    if response.status == 200:
                        geo_data = await response.json()
                        if geo_data.get("results"):
                            location = geo_data["results"][0]
                            lat, lon = location["latitude"], location["longitude"]
                            
                            # Get weather data
                            weather_url = f"{self.fallback_url}/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code&timezone=auto"
                            
                            async with session.get(weather_url) as weather_response:
                                if weather_response.status == 200:
                                    weather_data = await weather_response.json()
                                    current = weather_data.get("current", {})
                                    
                                    return {
                                        "location": f"{location['name']}, {location.get('country', 'Unknown')}",
                                        "latitude": lat,
                                        "longitude": lon,
                                        "temperature": f"{current.get('temperature_2m', 'N/A')}°C",
                                        "humidity": f"{current.get('relative_humidity_2m', 'N/A')}%",
                                        "wind_speed": f"{current.get('wind_speed_10m', 'N/A')} km/h",
                                        "weather_code": current.get('weather_code', 'N/A'),
                                        "timestamp": current.get('time', 'N/A')
                                    }
                        else:
                            return {"error": f"Location '{city}' not found"}
                    else:
                        return {"error": f"Failed to fetch location data: {response.status}"}
                        
        except Exception as e:
            return {"error": f"Error fetching weather data: {str(e)}"}

    async def get_weather_forecast(self, city: str, days: int = 5) -> Dict[str, Any]:
        """Get weather forecast for a specified location."""
        try:
            if days > 16:  # API limit
                days = 16
                
            # Get coordinates for the city
            geocoding_url = f"https://geocoding-api.open-meteo.com/v1/search?name={city}&count=1"
            
            async with aiohttp.ClientSession(connector=aiohttp.TCPConnector(ssl=self.ssl_context)) as session:
                async with session.get(geocoding_url) as response:
                    if response.status == 200:
                        geo_data = await response.json()
                        if geo_data.get("results"):
                            location = geo_data["results"][0]
                            lat, lon = location["latitude"], location["longitude"]
                            
                            # Get forecast data
                            forecast_url = f"{self.fallback_url}/forecast?latitude={lat}&longitude={lon}&daily=temperature_2m_max,temperature_2m_min,precipitation_sum,wind_speed_10m_max,weather_code&timezone=auto&forecast_days={days}"
                            
                            async with session.get(forecast_url) as forecast_response:
                                if forecast_response.status == 200:
                                    forecast_data = await forecast_response.json()
                                    daily = forecast_data.get("daily", {})
                                    
                                    forecast_list = []
                                    for i in range(len(daily.get("time", []))):
                                        forecast_list.append({
                                            "date": daily["time"][i],
                                            "temperature_max": f"{daily['temperature_2m_max'][i]}°C",
                                            "temperature_min": f"{daily['temperature_2m_min'][i]}°C",
                                            "precipitation": f"{daily['precipitation_sum'][i]}mm",
                                            "wind_speed": f"{daily['wind_speed_10m_max'][i]} km/h",
                                            "weather_code": daily['weather_code'][i]
                                        })
                                    
                                    return {
                                        "location": f"{location['name']}, {location.get('country', 'Unknown')}",
                                        "latitude": lat,
                                        "longitude": lon,
                                        "forecast_days": days,
                                        "forecast": forecast_list
                                    }
                        else:
                            return {"error": f"Location '{city}' not found"}
                    else:
                        return {"error": f"Failed to fetch location data: {response.status}"}
                        
        except Exception as e:
            return {"error": f"Error fetching forecast data: {str(e)}"}

    async def get_weather_alerts(self, city: str) -> Dict[str, Any]:
        """Get weather alerts and warnings for a specified location."""
        try:
            # For demo purposes, simulating weather alerts
            # In production, you would integrate with a real weather alert service
            
            # Get coordinates for the city first
            geocoding_url = f"https://geocoding-api.open-meteo.com/v1/search?name={city}&count=1"
            
            async with aiohttp.ClientSession(connector=aiohttp.TCPConnector(ssl=self.ssl_context)) as session:
                async with session.get(geocoding_url) as response:
                    if response.status == 200:
                        geo_data = await response.json()
                        if geo_data.get("results"):
                            location = geo_data["results"][0]
                            
                            # Simulate checking for alerts based on current conditions
                            weather_url = f"{self.fallback_url}/forecast?latitude={location['latitude']}&longitude={location['longitude']}&current=temperature_2m,wind_speed_10m&timezone=auto"
                            
                            async with session.get(weather_url) as weather_response:
                                if weather_response.status == 200:
                                    weather_data = await weather_response.json()
                                    current = weather_data.get("current", {})
                                    
                                    alerts = []
                                    temp = current.get('temperature_2m', 0)
                                    wind_speed = current.get('wind_speed_10m', 0)
                                    
                                    # Simple alert logic for demo
                                    if temp > 35:
                                        alerts.append({
                                            "type": "Heat Warning",
                                            "severity": "High",
                                            "description": f"Extreme heat warning - Temperature: {temp}°C"
                                        })
                                    elif temp < -10:
                                        alerts.append({
                                            "type": "Cold Warning", 
                                            "severity": "Medium",
                                            "description": f"Extreme cold warning - Temperature: {temp}°C"
                                        })
                                    
                                    if wind_speed > 50:
                                        alerts.append({
                                            "type": "Wind Warning",
                                            "severity": "High", 
                                            "description": f"High wind warning - Speed: {wind_speed} km/h"
                                        })
                                    
                                    return {
                                        "location": f"{location['name']}, {location.get('country', 'Unknown')}",
                                        "active_alerts": len(alerts),
                                        "alerts": alerts if alerts else [{"message": "No active weather alerts"}]
                                    }
                        else:
                            return {"error": f"Location '{city}' not found"}
                    else:
                        return {"error": f"Failed to fetch location data: {response.status}"}
                        
        except Exception as e:
            return {"error": f"Error fetching weather alerts: {str(e)}"}

    # Sync wrappers for the async methods
    def get_current_weather_sync(self, city: str, country: str = "US") -> Dict[str, Any]:
        """Synchronous wrapper for get_current_weather."""
        try:
            loop = asyncio.get_event_loop()
            if loop.is_running():
                # We're in an async context, need to use different approach
                import concurrent.futures
                import threading
                
                def run_in_thread():
                    new_loop = asyncio.new_event_loop()
                    asyncio.set_event_loop(new_loop)
                    try:
                        return new_loop.run_until_complete(self.get_current_weather(city, country))
                    finally:
                        new_loop.close()
                
                with concurrent.futures.ThreadPoolExecutor() as executor:
                    future = executor.submit(run_in_thread)
                    return future.result()
            else:
                return asyncio.run(self.get_current_weather(city, country))
        except RuntimeError:
            return asyncio.run(self.get_current_weather(city, country))

    def get_weather_forecast_sync(self, city: str, days: int = 5) -> Dict[str, Any]:
        """Synchronous wrapper for get_weather_forecast."""
        try:
            loop = asyncio.get_event_loop()
            if loop.is_running():
                import concurrent.futures
                import threading
                
                def run_in_thread():
                    new_loop = asyncio.new_event_loop()
                    asyncio.set_event_loop(new_loop)
                    try:
                        return new_loop.run_until_complete(self.get_weather_forecast(city, days))
                    finally:
                        new_loop.close()
                
                with concurrent.futures.ThreadPoolExecutor() as executor:
                    future = executor.submit(run_in_thread)
                    return future.result()
            else:
                return asyncio.run(self.get_weather_forecast(city, days))
        except RuntimeError:
            return asyncio.run(self.get_weather_forecast(city, days))

    def get_weather_alerts_sync(self, city: str) -> Dict[str, Any]:
        """Synchronous wrapper for get_weather_alerts."""
        try:
            loop = asyncio.get_event_loop()
            if loop.is_running():
                import concurrent.futures
                import threading
                
                def run_in_thread():
                    new_loop = asyncio.new_event_loop()
                    asyncio.set_event_loop(new_loop)
                    try:
                        return new_loop.run_until_complete(self.get_weather_alerts(city))
                    finally:
                        new_loop.close()
                
                with concurrent.futures.ThreadPoolExecutor() as executor:
                    future = executor.submit(run_in_thread)
                    return future.result()
            else:
                return asyncio.run(self.get_weather_alerts(city))
        except RuntimeError:
            return asyncio.run(self.get_weather_alerts(city))