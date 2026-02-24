"""
Instrumentation Injector
Handles injection of LangTrace configuration into agent code.
"""

import logging
import os
from pathlib import Path

logger = logging.getLogger(__name__)

class InstrumentationInjector:
    """Handles LangTrace instrumentation injection"""
    
    def __init__(self):
        self.langtrace_config_template = self._get_langtrace_config_template()
    
    def inject_langtrace_config(self, agent_temp_path, agent_name):
        """Create langtrace_config.py file and inject import at top of main.py"""
        if os.getenv("LANGTRACE_ENABLED", "false").lower() not in ("1", "true", "yes"):
            logger.info(f"Langtrace disabled; skipping injection for {agent_name}")
            return False
        # Find the correct directory for main.py
        main_py_paths = [
            agent_temp_path / "src" / "main.py",
            agent_temp_path / "main.py",
            agent_temp_path / "__main__.py",
            agent_temp_path / "src" / "__main__.py",
        ]
        
        main_py_path = None
        config_dir = None
        
        for path in main_py_paths:
            if path.exists():
                main_py_path = path
                config_dir = path.parent
                break
        
        if not main_py_path:
            logger.warning(f"No main.py found for {agent_name}, skipping Langtrace injection...")
            return False
        
        # Create langtrace_config.py in the same directory as main.py
        config_file_path = config_dir / "langtrace_config.py"
        config_file_path.write_text(self.langtrace_config_template)
        logger.info(f"Created langtrace_config.py for {agent_name}")
        
        # Read current main.py content
        original_content = main_py_path.read_text()
        
        # Check if langtrace_config is already imported
        if "import langtrace_config" in original_content:
            logger.info(f"Langtrace config already imported in {agent_name} main.py, skipping injection...")
            return True
        
        # Find the right place to inject - after existing imports but before other code
        lines = original_content.split('\n')
        
        # Find the best insertion point (enhanced for A2A agents)
        insert_index = 0
        found_imports = False
        
        for i, line in enumerate(lines):
            stripped_line = line.strip()
            
            # Skip shebang line for A2A agents
            if stripped_line.startswith('#!'):
                insert_index = i + 1
                continue
            
            # Skip encoding declarations
            if stripped_line.startswith('# -*- coding:') or stripped_line.startswith('# coding:'):
                insert_index = i + 1
                continue
                
            # Skip module docstrings
            if stripped_line.startswith('"""') or stripped_line.startswith("'''"):
                # Find the end of docstring
                quote_type = stripped_line[:3]
                if stripped_line.count(quote_type) >= 2:  # Single line docstring
                    insert_index = i + 1
                    continue
                else:  # Multi-line docstring
                    for j in range(i + 1, len(lines)):
                        if quote_type in lines[j]:
                            insert_index = j + 1
                            break
                    continue
            
            # Track imports and continue after them (handle multi-line imports)
            if (stripped_line.startswith('import ') or 
                stripped_line.startswith('from ') or
                stripped_line.startswith('__') and stripped_line.endswith('__')):  # __future__ imports etc
                found_imports = True
                
                # For multi-line imports, find the end of the import statement
                if ('(' in stripped_line and ')' not in stripped_line) or stripped_line.endswith('\\'):
                    # This is a multi-line import, find where it ends
                    for j in range(i + 1, len(lines)):
                        next_line = lines[j].strip()
                        # For parentheses style: look for closing )
                        if '(' in stripped_line and ')' in next_line:
                            insert_index = j + 1
                            break
                        # For backslash style: look for line not ending with \
                        elif stripped_line.endswith('\\') and not next_line.endswith('\\') and next_line:
                            insert_index = j + 1
                            break
                    continue
                else:
                    # Single line import
                    insert_index = i + 1
                    continue
            
            # Skip comments and empty lines after imports
            if (stripped_line.startswith('#') or stripped_line == ''):
                if found_imports:  # Only advance if we've seen imports
                    insert_index = i + 1
                continue
                
            # First non-import, non-comment, non-empty line
            if stripped_line:
                break
        
        # Insert the import at the right position
        import_line = "import langtrace_config  # Auto-injected for observability"
        lines.insert(insert_index, import_line)
        
        # Write back the modified content
        modified_content = '\n'.join(lines)
        main_py_path.write_text(modified_content)
        logger.info(f"Injected langtrace_config import into {agent_name} main.py at line {insert_index + 1}")
        
        return True
    
    def _get_langtrace_config_template(self):
        """Get the LangTrace configuration template"""
        return '''import os
from langtrace_python_sdk import langtrace

# Get environment variables
LANGTRACE_API_KEY = os.getenv("LANGTRACE_API_KEY")
LANGTRACE_API_HOST = os.getenv("LANGTRACE_API_HOST", "http://localhost:3000/api/trace")

print(f"Langtrace Config - API Key: {'***' + LANGTRACE_API_KEY[-8:] if LANGTRACE_API_KEY else 'NOT_SET'}")
print(f"Langtrace Config - API Host: {LANGTRACE_API_HOST}")

if LANGTRACE_API_KEY:
    try:
        # Initialize Langtrace
        langtrace.init(
            api_key=LANGTRACE_API_KEY,
            api_host=LANGTRACE_API_HOST
        )
        print("Langtrace core initialized")
        
        # Comprehensive Instrumentations
        instrumentations = [
            # Core LangChain & OpenAI - Use Langtrace instrumentation first
            ("langtrace_python_sdk.instrumentation.langchain", "LangchainInstrumentation", "LangChain"),
            ("langtrace_python_sdk.instrumentation.openai", "OpenAIInstrumentation", "OpenAI"),
            
            # LLM Libraries
            ("langtrace_python_sdk.instrumentation.anthropic", "AnthropicInstrumentation", "Anthropic"),
            ("langtrace_python_sdk.instrumentation.google_generativeai", "GoogleGenerativeAIInstrumentation", "Google Gemini"),
            
            # AI Frameworks  
            ("langtrace_python_sdk.instrumentation.crewai", "CrewAIInstrumentation", "CrewAI"),
            
            # Web Frameworks
            ("opentelemetry.instrumentation.fastapi", "FastAPIInstrumentor", "FastAPI"),
            ("opentelemetry.instrumentation.django", "DjangoInstrumentor", "Django"),
            ("opentelemetry.instrumentation.flask", "FlaskInstrumentor", "Flask"),
            
            # HTTP Clients
            ("opentelemetry.instrumentation.requests", "RequestsInstrumentor", "Requests"),
            ("opentelemetry.instrumentation.httpx", "HTTPXInstrumentor", "HTTPX"),
            ("opentelemetry.instrumentation.aiohttp_client", "AioHttpClientInstrumentor", "AioHTTP Client"),
            ("opentelemetry.instrumentation.boto3sqs", "Boto3SQSInstrumentor", "AWS SQS"),
            
            # Databases
            ("opentelemetry.instrumentation.pymongo", "PymongoInstrumentor", "MongoDB"),
            ("opentelemetry.instrumentation.psycopg2", "Psycopg2Instrumentor", "PostgreSQL"),
            ("opentelemetry.instrumentation.sqlalchemy", "SQLAlchemyInstrumentor", "SQLAlchemy"),
            ("opentelemetry.instrumentation.redis", "RedisInstrumentor", "Redis"),
            
            # Vector Databases
            ("langtrace_python_sdk.instrumentation.pinecone", "PineconeInstrumentation", "Pinecone"),
            ("langtrace_python_sdk.instrumentation.chromadb", "ChromaInstrumentation", "ChromaDB"),
        ]
        
        instrumented_count = 0
        for module_path, class_name, display_name in instrumentations:
            try:
                module = __import__(module_path, fromlist=[class_name])
                instrumentor_class = getattr(module, class_name)
                instrumentor_class().instrument()
                print(f"{display_name} instrumentation completed")
                instrumented_count += 1
            except ImportError:
                # Module not available, skip silently
                pass
            except Exception as e:
                print(f"{display_name} instrumentation failed: {e}")
        
        print(f"Langtrace initialized with {instrumented_count} instrumentations for agent: {os.getenv('OTEL_SERVICE_NAME', 'unknown')}")
        
        # Setup OpenTelemetry context injection using proper span processing
        try:
            from opentelemetry import trace, context
            from opentelemetry.sdk.trace import TracerProvider, SpanProcessor
            from opentelemetry.sdk.trace.export import SpanExporter
            from opentelemetry.trace import Status, StatusCode
            import re
            import json
            from urllib.parse import parse_qs
            
            class SessionContextSpanProcessor(SpanProcessor):
                """Span processor to inject session context into spans using custom exporter"""
                
                def __init__(self):
                    self.agent_name = os.getenv('OTEL_SERVICE_NAME', 'unknown-agent')
                    print(f"SessionContextSpanProcessor initialized for agent: {self.agent_name}")
                
                def on_start(self, span, parent_context):
                    """Called when span starts - inject session context if available"""
                    try:
                        # Method 1: Get session context from OpenTelemetry context using proper API
                        from opentelemetry.context import get_value
                        session_id = get_value("nasiko.session_id", parent_context)
                        
                        if session_id:
                            # Set session attributes on the span
                            span.set_attribute("session.id", session_id)
                            span.set_attribute("session_id", session_id)
                            span.set_attribute("agent.name", self.agent_name)
                            span.set_attribute("agent.type", "upload")
                            
                            print(f"Session context injected into span: {session_id} for {self.agent_name}")
                            
                    except Exception as e:
                        print(f"Error in SessionContextSpanProcessor.on_start: {e}")
                
                def on_end(self, span):
                    """Called when span ends - spans are read-only here"""
                    pass
                
                def shutdown(self):
                    """Called on shutdown"""
                    pass
                
                def force_flush(self, timeout_millis=30000):
                    """Called to force flush"""
                    pass
                    
            
            # Add the session context span processor to the tracer provider
            tracer_provider = trace.get_tracer_provider()
            if hasattr(tracer_provider, 'add_span_processor'):
                session_processor = SessionContextSpanProcessor()
                tracer_provider.add_span_processor(session_processor)
                print("SessionContextSpanProcessor added to tracer provider")
            else:
                print("Warning: TracerProvider doesn't support add_span_processor")
            
            # Monkey patch LangTrace's OTLP exporter to inject session context
            try:
                from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
                
                # Store original export and _export_http methods
                original_export = OTLPSpanExporter.export
                original_export_http = OTLPSpanExporter._export_http if hasattr(OTLPSpanExporter, '_export_http') else None
                
                def patched_export(self, spans):
                    """Enhanced export method that extracts session_id from LangChain metadata"""
                    try:
                        modified_spans = []
                        for span in spans:
                            # Skip if session_id already set
                            if hasattr(span, 'attributes') and (span.attributes.get("session_id") or span.attributes.get("session.id")):
                                modified_spans.append(span)
                                continue
                            
                            session_id = None
                            
                            # Extract session_id from LangChain metadata  
                            if hasattr(span, 'attributes'):
                                langchain_metadata = span.attributes.get("langchain.metadata")
                                if langchain_metadata:
                                    try:
                                        import json
                                        metadata = json.loads(langchain_metadata)
                                        if isinstance(metadata, dict) and "metadata" in metadata:
                                            nested_metadata = metadata["metadata"]
                                            if isinstance(nested_metadata, dict) and "session_id" in nested_metadata:
                                                session_id = nested_metadata["session_id"]
                                                print(f"[SessionExtractor] Extracted session_id from LangChain metadata: {session_id}")
                                    except Exception as e:
                                        print(f"[SessionExtractor] Error parsing LangChain metadata: {e}")
                                
                                # Extract session_id from LangChain inputs if metadata didn't work
                                if not session_id:
                                    langchain_inputs = span.attributes.get("langchain.inputs")
                                    if langchain_inputs:
                                        try:
                                            import json
                                            inputs = json.loads(langchain_inputs)
                                            if isinstance(inputs, list) and len(inputs) > 1:
                                                for item in inputs:
                                                    if isinstance(item, dict):
                                                        if "metadata" in item and isinstance(item["metadata"], dict):
                                                            if "session_id" in item["metadata"]:
                                                                session_id = item["metadata"]["session_id"]
                                                                print(f"[SessionExtractor] Extracted session_id from LangChain inputs: {session_id}")
                                                                break
                                                        if "configurable" in item and isinstance(item["configurable"], dict):
                                                            if "session_id" in item["configurable"]:
                                                                session_id = item["configurable"]["session_id"]
                                                                print(f"[SessionExtractor] Extracted session_id from LangChain configurable: {session_id}")
                                                                break
                                        except Exception as e:
                                            print(f"[SessionExtractor] Error parsing LangChain inputs: {e}")
                            
                            # Inject session context if found
                            if session_id and hasattr(span, 'attributes'):
                                # Patch the span attributes directly
                                from opentelemetry.sdk.trace import ReadableSpan
                                if isinstance(span, ReadableSpan):
                                    new_attributes = dict(span.attributes) if span.attributes else {}
                                    new_attributes.update({
                                        "session.id": session_id,
                                        "session_id": session_id,
                                        "agent.name": os.getenv('OTEL_SERVICE_NAME', 'unknown-agent'),
                                        "agent.type": "upload"
                                    })
                                    span._attributes = new_attributes
                                    print(f"[SessionExtractor] Session context injected into span: {session_id}")
                            
                            modified_spans.append(span)
                        
                        # Call original export with modified spans
                        return original_export(self, modified_spans)
                        
                    except Exception as e:
                        print(f"[SessionExtractor] Error in patched export: {e}")
                        # Fallback to original export
                        return original_export(self, spans)
                
                # Apply the monkey patch
                OTLPSpanExporter.export = patched_export
                print("LangTrace OTLP exporter monkey patched for session extraction")
                
            except ImportError as e:
                print(f"OTLP exporter not available for monkey patching: {e}")
            except Exception as e:
                print(f"Failed to monkey patch OTLP exporter: {e}")
            
            # Setup FastAPI context injection using proper OpenTelemetry patterns
            import fastapi
            from starlette.middleware.base import BaseHTTPMiddleware
            
            class OpenTelemetrySessionMiddleware(BaseHTTPMiddleware):
                """Middleware to extract session_id and set OpenTelemetry context"""
                
                async def dispatch(self, request, call_next):
                    session_id = None
                    
                    try:
                        # Extract session_id from various sources WITHOUT consuming request body
                        
                        # 1. Check path parameters first (for /history/{session_id})
                        if hasattr(request, 'path_params') and "session_id" in request.path_params:
                            session_id = request.path_params["session_id"]
                        
                        # 2. Check query parameters
                        elif "session_id" in request.query_params:
                            session_id = request.query_params["session_id"]
                        
                        # 3. For JSON-RPC requests (A2A agents) - extract sessionId from metadata
                        elif request.method == "POST":
                            content_type = request.headers.get("content-type", "").lower()
                            
                            if "application/json" in content_type:
                                try:
                                    # Read JSON body for A2A agents
                                    body = await request.body()
                                    if body:
                                        import json
                                        json_data = json.loads(body.decode('utf-8'))
                                        print(f"[SessionExtractor] Processing JSON-RPC request")
                                        
                                        # Check for JSON-RPC format with metadata.sessionId
                                        if (json_data.get("jsonrpc") == "2.0" and 
                                            "params" in json_data and 
                                            isinstance(json_data["params"], dict)):
                                            
                                            metadata = json_data["params"].get("metadata", {})
                                            if isinstance(metadata, dict) and "sessionId" in metadata:
                                                session_id = metadata["sessionId"]
                                                print(f"[SessionExtractor] ✓ Extracted sessionId from JSON-RPC metadata: {session_id}")
                                            else:
                                                print(f"[SessionExtractor] No sessionId in metadata: {list(metadata.keys()) if isinstance(metadata, dict) else 'not a dict'}")
                                        else:
                                            print(f"[SessionExtractor] Not a valid JSON-RPC request or missing params")
                                        
                                        # Reconstruct the request with the body for downstream processing
                                        from starlette.requests import Request
                                        request._body = body
                                        
                                except Exception as e:
                                    print(f"[SessionExtractor] Error parsing JSON-RPC for sessionId: {e}")
                                    import traceback
                                    traceback.print_exc()
                        
                        # 4. For multipart/form-data requests (traditional FastAPI Form data)
                        elif (request.method == "POST" and 
                              request.headers.get("content-type", "").startswith("multipart/form-data")):
                            # For multipart requests, session extraction deferred to span processing
                            print("Multipart form-data detected, session extraction deferred to span processing")
                        
                        # 5. For URL-encoded form data (traditional agents)
                        elif (request.method == "POST" and 
                              request.headers.get("content-type", "").startswith("application/x-www-form-urlencoded")):
                            # Extract session_id from form data
                            try:
                                body = await request.body()
                                if body:
                                    form_data = parse_qs(body.decode('utf-8'))
                                    if 'session_id' in form_data:
                                        session_id = form_data['session_id'][0]
                                        
                                    # Reconstruct the request with the body
                                    from starlette.requests import Request
                                    request._body = body
                                    
                            except Exception as e:
                                print(f"Error parsing form data for session_id: {e}")
                        
                        # Set OpenTelemetry context if session_id found
                        if session_id:
                            # Create new context with session_id
                            ctx = context.set_value("nasiko.session_id", session_id)
                            token = context.attach(ctx)
                            
                            try:
                                print(f"OpenTelemetry context set: session_id={session_id}")
                                # Continue with the request in the new context
                                response = await call_next(request)
                                return response
                            finally:
                                # Always detach context
                                context.detach(token)
                        else:
                            # No session_id found in middleware, continue normally
                            # For multipart requests, session extraction happens later
                            response = await call_next(request)
                            return response
                            
                    except Exception as e:
                        print(f"Error in OpenTelemetrySessionMiddleware: {e}")
                        # Continue with request even if session extraction fails
                        response = await call_next(request)
                        return response
            
            # Monkey patch both FastAPI and Starlette to auto-add our OpenTelemetry middleware
            
            # FastAPI monkey patch
            original_fastapi_init = fastapi.FastAPI.__init__
            
            def patched_fastapi_init(self, *args, **kwargs):
                # Call original init
                original_fastapi_init(self, *args, **kwargs)
                # Add our OpenTelemetry session context middleware
                try:
                    self.add_middleware(OpenTelemetrySessionMiddleware)
                    print(f"OpenTelemetry session context middleware auto-added to FastAPI app")
                except Exception as e:
                    print(f"Failed to add OpenTelemetry session context middleware: {e}")
            
            fastapi.FastAPI.__init__ = patched_fastapi_init
            
            # Starlette monkey patch (for A2A agents)
            try:
                from starlette.applications import Starlette
                original_starlette_init = Starlette.__init__
                
                def patched_starlette_init(self, *args, **kwargs):
                    # Call original init
                    original_starlette_init(self, *args, **kwargs)
                    # Add our OpenTelemetry session context middleware
                    try:
                        self.add_middleware(OpenTelemetrySessionMiddleware)
                        print(f"[A2A] OpenTelemetry session context middleware auto-added to Starlette app")
                    except Exception as e:
                        print(f"[A2A] Failed to add OpenTelemetry session context middleware: {e}")
                
                Starlette.__init__ = patched_starlette_init
                print("OpenTelemetry session context injection enabled for both FastAPI and Starlette")
                
            except ImportError:
                print("Starlette not available, only FastAPI middleware enabled")
                print("OpenTelemetry session context injection enabled for FastAPI")
            
        except ImportError as e:
            print(f"OpenTelemetry modules not available for session context injection: {e}")
        except Exception as e:
            print(f"Failed to setup OpenTelemetry session context injection: {e}")
        
    except Exception as e:
        print(f"Failed to initialize Langtrace: {e}")
else:
    print("LANGTRACE_API_KEY not set, Langtrace not initialized")

'''
