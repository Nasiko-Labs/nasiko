"""
MCP Detection Handler - Detects artifact type (agent vs MCP server)
"""

from fastapi import HTTPException, status, UploadFile
from .base_handler import BaseHandler
from typing import Dict, Any
import tempfile
import zipfile
import os
from pathlib import Path


class MCPDetectionHandler(BaseHandler):
    """Handler for MCP artifact detection"""

    def __init__(self, service, logger):
        super().__init__(service, logger)

    async def detect_artifact_type(self, file: UploadFile) -> Dict[str, Any]:
        """
        Detect if uploaded artifact is an agent or MCP server
        
        Returns:
            {
                "artifact_type": "agent" | "mcp_server",
                "confidence": float,
                "detected_patterns": list,
                "tools_found": int,
                "manifest_preview": dict (if MCP server)
            }
        """
        try:
            self.log_info("Detecting artifact type", filename=file.filename)
            
            # Extract to temp directory
            temp_dir = tempfile.mkdtemp(prefix="mcp_detect_")
            
            try:
                # Save and extract zip
                zip_path = os.path.join(temp_dir, "upload.zip")
                content = await file.read()
                
                with open(zip_path, "wb") as f:
                    f.write(content)
                
                if not zipfile.is_zipfile(zip_path):
                    raise HTTPException(
                        status_code=status.HTTP_400_BAD_REQUEST,
                        detail="Invalid zip file"
                    )
                
                with zipfile.ZipFile(zip_path, 'r') as zip_ref:
                    zip_ref.extractall(temp_dir)
                
                os.remove(zip_path)
                
                # Analyze extracted files
                detection_result = await self._analyze_artifact(temp_dir)
                
                return {
                    "success": True,
                    "data": detection_result,
                    "status_code": 200,
                    "message": f"Detected as {detection_result['artifact_type']}"
                }
                
            finally:
                # Cleanup
                import shutil
                if os.path.exists(temp_dir):
                    shutil.rmtree(temp_dir)
                    
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("detect_artifact_type", e)

    async def _analyze_artifact(self, directory: str) -> Dict[str, Any]:
        """Analyze directory to determine artifact type"""
        
        detected_patterns = []
        confidence = 0.0
        artifact_type = "unknown"
        tools_found = 0
        manifest_preview = None
        
        # Search for Python files
        python_files = list(Path(directory).rglob("*.py"))
        
        mcp_indicators = 0
        agent_indicators = 0
        
        for py_file in python_files:
            try:
                content = py_file.read_text(encoding='utf-8')
                
                # Check for MCP patterns
                if 'from mcp' in content or 'import mcp' in content:
                    mcp_indicators += 3
                    detected_patterns.append(f"MCP SDK import in {py_file.name}")
                
                if 'FastMCP' in content or '@mcp.tool' in content:
                    mcp_indicators += 5
                    detected_patterns.append(f"FastMCP usage in {py_file.name}")
                    
                    # Try to count tools
                    tools_found += content.count('@mcp.tool')
                    tools_found += content.count('@app.tool')
                
                # Check for agent patterns
                if 'from langchain' in content or 'import langchain' in content:
                    agent_indicators += 2
                    detected_patterns.append(f"LangChain import in {py_file.name}")
                
                if 'from crewai' in content or 'import crewai' in content:
                    agent_indicators += 2
                    detected_patterns.append(f"CrewAI import in {py_file.name}")
                
            except Exception as e:
                self.log_error(f"Error analyzing {py_file}", e)
                continue
        
        # Check for existing manifests
        mcp_manifest_path = Path(directory) / "mcp_manifest.json"
        agentcard_path = Path(directory) / "AgentCard.json"
        
        if mcp_manifest_path.exists():
            mcp_indicators += 10
            detected_patterns.append("mcp_manifest.json found")
            
            # Load manifest preview
            try:
                import json
                with open(mcp_manifest_path) as f:
                    manifest_preview = json.load(f)
                    if 'tools' in manifest_preview:
                        tools_found = len(manifest_preview['tools'])
            except:
                pass
        
        if agentcard_path.exists():
            agent_indicators += 5
            detected_patterns.append("AgentCard.json found")
        
        # Determine artifact type
        if mcp_indicators > agent_indicators:
            artifact_type = "mcp_server"
            confidence = min(mcp_indicators / 15.0, 1.0)
        elif agent_indicators > mcp_indicators:
            artifact_type = "agent"
            confidence = min(agent_indicators / 10.0, 1.0)
        else:
            artifact_type = "unknown"
            confidence = 0.0
        
        # Generate manifest preview if MCP server and no manifest exists
        if artifact_type == "mcp_server" and not manifest_preview:
            manifest_preview = await self._generate_manifest_preview(directory)
        
        return {
            "artifact_type": artifact_type,
            "confidence": round(confidence, 2),
            "detected_patterns": detected_patterns,
            "tools_found": tools_found,
            "manifest_preview": manifest_preview,
            "analysis": {
                "mcp_score": mcp_indicators,
                "agent_score": agent_indicators,
                "python_files_analyzed": len(python_files)
            }
        }

    async def _generate_manifest_preview(self, directory: str) -> Dict[str, Any]:
        """Generate a preview manifest using the MCP manifest generator"""
        try:
            from app.utils.agentcard_generator.mcp import MCPManifestGenerator
            
            generator = MCPManifestGenerator()
            
            # Find main Python file
            main_files = [
                Path(directory) / "main.py",
                Path(directory) / "src" / "main.py",
                Path(directory) / "server.py",
            ]
            
            for main_file in main_files:
                if main_file.exists():
                    result = generator.extract_capabilities(str(main_file))
                    if result.get("status") == "success":
                        return result.get("manifest")
            
            return None
            
        except Exception as e:
            self.log_error("Failed to generate manifest preview", e)
            return None
