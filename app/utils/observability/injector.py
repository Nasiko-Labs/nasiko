import os
import shutil
import ast
import astor
from typing import Optional, List
import logging

from .config import ObservabilityConfig

logger = logging.getLogger(__name__)

class TracingInjector:
    """Handles automatic injection of observability code into agent containers"""
    
    def __init__(self, observability_source_path: str = "/app/utils/observability"):
        self.observability_source = observability_source_path
        self.config = ObservabilityConfig()
        
    def inject_into_agent(self, agent_code_path: str, agent_name: str) -> bool:
        """
        Complete observability injection process
        
        Args:
            agent_code_path: Path to agent source code
            agent_name: Name of the agent for project identification
            
        Returns:
            bool: Success/failure status
        """
        if not self.config.is_tracing_enabled():
            logger.info(f"Tracing disabled, skipping injection for {agent_name}")
            return True
            
        if not self.config.get_injection_enabled():
            logger.info(f"Injection disabled, skipping for {agent_name}")
            return True
            
        try:
            logger.info(f"🔄 Starting observability injection for {agent_name}")
            
            # 1. Copy observability module
            self._copy_observability_module(agent_code_path)
            
            # 2. Find and modify main entry point
            main_file = self._find_main_file(agent_code_path)
            self._inject_tracing_code(main_file, agent_name)
            
            # 3. Update dependencies
            self._update_requirements(agent_code_path)
            
            # 4. Update Dockerfile if needed
            self._update_dockerfile(agent_code_path)
            
            logger.info(f"✅ Observability injection completed for {agent_name}")
            return True
            
        except Exception as e:
            logger.error(f"❌ Failed to inject observability for {agent_name}: {e}")
            return False
    
    def _copy_observability_module(self, agent_code_path: str):
        """Copy observability module into agent directory"""
        # Create utils directory structure
        utils_dir = os.path.join(agent_code_path, "utils")
        os.makedirs(utils_dir, exist_ok=True)
        
        # Create utils/__init__.py if it doesn't exist
        utils_init = os.path.join(utils_dir, "__init__.py")
        if not os.path.exists(utils_init):
            with open(utils_init, 'w') as f:
                f.write('# Utils package\n')
        
        # Copy observability module (exclude injector and config files used only for build-time injection)
        dest_path = os.path.join(utils_dir, "observability")
        if os.path.exists(dest_path):
            shutil.rmtree(dest_path)
        
        # Copy observability module but exclude build-time only files
        def ignore_patterns(dir, files):
            # Exclude injector.py and config.py since they're only needed during build
            exclude = {'injector.py', 'config.py', '__pycache__'}
            return [f for f in files if f in exclude]
            
        shutil.copytree(self.observability_source, dest_path, ignore=ignore_patterns)
        logger.info(f"📁 Copied observability module to {dest_path} (excluding build-time files)")
    
    def _find_main_file(self, agent_code_path: str) -> str:
        """Find the main entry point file"""
        candidates = [
            "__main__.py", 
            "main.py", 
            "app.py",
            "run.py"
        ]
        
        # Check direct candidates first
        for candidate in candidates:
            full_path = os.path.join(agent_code_path, candidate)
            if os.path.exists(full_path):
                return full_path
        
        # Also check in src/ subdirectory
        src_dir = os.path.join(agent_code_path, "src")
        if os.path.exists(src_dir):
            for candidate in candidates:
                full_path = os.path.join(src_dir, candidate)
                if os.path.exists(full_path):
                    return full_path
        
        # Fallback: search for files with uvicorn.run or FastAPI
        for root, dirs, files in os.walk(agent_code_path):
            for file in files:
                if file.endswith('.py'):
                    file_path = os.path.join(root, file)
                    try:
                        with open(file_path, 'r', encoding='utf-8') as f:
                            content = f.read()
                            if any(pattern in content for pattern in ['uvicorn.run', 'FastAPI(', 'Starlette(']):
                                return file_path
                    except (UnicodeDecodeError, PermissionError):
                        continue
        
        raise ValueError(f"No main entry point found in {agent_code_path}")
    
    def _inject_tracing_code(self, main_file: str, agent_name: str):
        """Inject tracing imports and bootstrap call using AST"""
        try:
            with open(main_file, 'r', encoding='utf-8') as f:
                source = f.read()
            
            tree = ast.parse(source)
            
            # Create import statement
            import_stmt = ast.ImportFrom(
                module='utils.observability.tracing_utils',
                names=[ast.alias('bootstrap_tracing', None)],
                level=0
            )
            
            # Read framework from AgentCard.json if available
            framework = self._get_agent_framework(main_file)
            
            # Create bootstrap call with framework
            bootstrap_keywords = [
                ast.keyword('project_name', ast.Constant(agent_name))
            ]
            if framework:
                bootstrap_keywords.append(
                    ast.keyword('framework', ast.Constant(framework))
                )
            
            bootstrap_call = ast.Expr(
                ast.Call(
                    func=ast.Name('bootstrap_tracing', ast.Load()),
                    args=[],
                    keywords=bootstrap_keywords
                )
            )
            
            # Find insertion points
            last_import_idx = self._find_last_import_index(tree)
            
            # Insert import after last import
            tree.body.insert(last_import_idx + 1, import_stmt)
            # Insert bootstrap call after import  
            tree.body.insert(last_import_idx + 2, bootstrap_call)
            
            # Write back modified code
            modified_source = astor.to_source(tree)
            with open(main_file, 'w', encoding='utf-8') as f:
                f.write(modified_source)
                
            logger.info(f"🔧 Injected tracing code into {main_file}")
            
        except Exception as e:
            logger.error(f"Failed to inject tracing code: {e}")
            raise
    
    def _find_last_import_index(self, tree: ast.AST) -> int:
        """Find the index of the last import statement"""
        last_import_idx = -1
        for i, node in enumerate(tree.body):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                last_import_idx = i
        return last_import_idx
    
    def _update_requirements(self, agent_code_path: str):
        """Add tracing dependencies to requirements.txt or pyproject.toml"""
        req_file = os.path.join(agent_code_path, "requirements.txt")
        pyproject_file = os.path.join(agent_code_path, "pyproject.toml")
        dependencies = self.config.get_required_dependencies()
        
        # Check for pyproject.toml first (modern Python projects)
        if os.path.exists(pyproject_file):
            self._update_pyproject_toml(pyproject_file, dependencies)
            logger.info(f"📦 Updated pyproject.toml with observability dependencies")
        elif os.path.exists(req_file):
            self._update_requirements_txt(req_file, dependencies)
            logger.info(f"📦 Updated requirements.txt with observability dependencies")
        else:
            # Create requirements.txt as fallback
            self._create_requirements_txt(req_file, dependencies)
            logger.info(f"📦 Created requirements.txt with observability dependencies")
    
    def _update_requirements_txt(self, req_file: str, dependencies: List[str]):
        """Update requirements.txt file"""
        with open(req_file, 'r') as f:
            existing = f.read().strip()
        
        with open(req_file, 'a') as f:
            f.write('\n\n# Observability dependencies\n')
            for dep in dependencies:
                f.write(f"{dep}\n")
    
    def _create_requirements_txt(self, req_file: str, dependencies: List[str]):
        """Create new requirements.txt file"""
        with open(req_file, 'w') as f:
            f.write("# Observability dependencies\n")
            for dep in dependencies:
                f.write(f"{dep}\n")
    
    def _update_pyproject_toml(self, pyproject_file: str, dependencies: List[str]):
        """Update pyproject.toml file with observability dependencies"""
        try:
            import toml
        except ImportError:
            # If toml is not available, create requirements.txt as fallback
            logger.warning("toml package not available, falling back to requirements.txt")
            req_file = os.path.join(os.path.dirname(pyproject_file), "requirements.txt")
            self._create_requirements_txt(req_file, dependencies)
            return
        
        try:
            # Read existing pyproject.toml
            with open(pyproject_file, 'r') as f:
                data = toml.load(f)
            
            # Ensure project.dependencies exists
            if 'project' not in data:
                data['project'] = {}
            if 'dependencies' not in data['project']:
                data['project']['dependencies'] = []
            
            # Check if dependencies are already present to avoid duplicates
            existing_deps = set(dep.split('>=')[0].split('==')[0] for dep in data['project']['dependencies'])
            new_deps = []
            
            for dep in dependencies:
                dep_name = dep.split('>=')[0].split('==')[0]
                if dep_name not in existing_deps:
                    new_deps.append(dep)
            
            # Add new dependencies
            if new_deps:
                data['project']['dependencies'].extend(new_deps)
                
                # Write back to file
                with open(pyproject_file, 'w') as f:
                    toml.dump(data, f)
                
                logger.info(f"Added {len(new_deps)} new observability dependencies to pyproject.toml")
            else:
                logger.info("All observability dependencies already present in pyproject.toml")
                
        except Exception as e:
            logger.error(f"Failed to update pyproject.toml: {e}")
            # Fallback to requirements.txt
            req_file = os.path.join(os.path.dirname(pyproject_file), "requirements.txt")
            self._create_requirements_txt(req_file, dependencies)
            logger.info("Created requirements.txt as fallback")
    
    def _update_dockerfile(self, agent_code_path: str):
        """Update Dockerfile to include utils directory and observability dependencies"""
        dockerfile_path = os.path.join(agent_code_path, "Dockerfile")
        
        if not os.path.exists(dockerfile_path):
            return
        
        with open(dockerfile_path, 'r') as f:
            content = f.read()
        
        lines = content.split('\n')
        new_lines = []
        updated_utils = False
        updated_deps = False
        i = 0
        
        while i < len(lines):
            line = lines[i]
            
            # Add utils copy after COPY src/ if not already present
            if (line.strip().startswith('COPY src/') and 
                not updated_utils and 
                'COPY utils/' not in content and 
                'COPY . /' not in content):
                new_lines.append(line)
                new_lines.append('COPY utils/ /app/utils/')
                updated_utils = True
                i += 1
                continue
            
            # Update pip install to include observability dependencies
            if (line.strip().startswith('RUN pip install') and 
                not updated_deps and
                'opentelemetry' not in line):
                
                # Collect all lines of the multiline pip install command
                pip_lines = [line]
                j = i + 1
                
                # Follow the multiline command (lines ending with \)
                while j < len(lines) and pip_lines[-1].rstrip().endswith('\\'):
                    pip_lines.append(lines[j])
                    j += 1
                
                # Get observability dependencies
                observability_deps = self._get_observability_dependencies()
                
                # If it's a multiline pip install, add dependencies before the last line
                if len(pip_lines) > 1:
                    # Add observability dependencies as new lines before the last line
                    for k, pip_line in enumerate(pip_lines[:-1]):
                        new_lines.append(pip_line)
                    
                    # Add observability dependencies
                    for dep in observability_deps:
                        new_lines.append(f'    "{dep}" \\')
                    
                    # Add the last line (usually just a package name without \)
                    last_line = pip_lines[-1].rstrip()
                    if not last_line.endswith('\\'):
                        new_lines.append(f'    {last_line}')
                    else:
                        new_lines.append(last_line)
                        
                else:
                    # Single line pip install
                    dep_string = ' '.join(f'"{dep}"' for dep in observability_deps)
                    new_line = line.rstrip() + ' ' + dep_string
                    new_lines.append(new_line)
                
                updated_deps = True
                i = j  # Skip the lines we've already processed
                continue
            
            new_lines.append(line)
            i += 1
        
        # Write updated Dockerfile
        with open(dockerfile_path, 'w') as f:
            f.write('\n'.join(new_lines))
            
        if updated_utils:
            logger.info(f"🐳 Updated Dockerfile to include utils directory")
        if updated_deps:
            logger.info(f"🐳 Updated Dockerfile to include observability dependencies")
    
    def _get_observability_dependencies(self) -> List[str]:
        """Get list of observability dependencies"""
        return self.config.get_required_dependencies()
    
    def _get_agent_framework(self, main_file: str) -> Optional[str]:
        """Read agent framework from AgentCard.json"""
        try:
            # Find the agent root directory (where AgentCard.json should be)
            agent_dir = os.path.dirname(main_file)
            
            # Check if main_file is in src/ subdirectory, if so go up one level
            if os.path.basename(agent_dir) == "src":
                agent_dir = os.path.dirname(agent_dir)
            
            agentcard_path = os.path.join(agent_dir, "AgentCard.json")
            
            if os.path.exists(agentcard_path):
                import json
                with open(agentcard_path, 'r', encoding='utf-8') as f:
                    agentcard = json.load(f)
                    
                framework = agentcard.get('agentFramework')
                if framework:
                    logger.info(f"📋 Detected agent framework: {framework}")
                    return framework.lower()  # Normalize to lowercase
                else:
                    logger.info("📋 No agentFramework field found in AgentCard.json")
            else:
                logger.info("📋 AgentCard.json not found, will use default instrumentors")
                
        except Exception as e:
            logger.warning(f"Failed to read agent framework from AgentCard: {e}")
            
        return None
    
    def validate_injection(self, agent_code_path: str) -> bool:
        """Validate that injection was successful"""
        try:
            # Check if observability module exists
            obs_path = os.path.join(agent_code_path, "utils", "observability")
            if not os.path.exists(obs_path):
                return False
            
            # Check if tracing_utils.py exists
            tracing_file = os.path.join(obs_path, "tracing_utils.py")
            if not os.path.exists(tracing_file):
                return False
                
            # Check if main file has bootstrap_tracing import
            main_file = self._find_main_file(agent_code_path)
            with open(main_file, 'r', encoding='utf-8') as f:
                content = f.read()
                if 'bootstrap_tracing' not in content:
                    return False
                    
            return True
            
        except Exception as e:
            logger.error(f"Validation failed: {e}")
            return False