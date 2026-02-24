from fastapi import HTTPException, status
from app.pkg.config.config import settings
from ..types import GetTracesRequest, GetTracesResponse
from .base_handler import BaseHandler
import requests
import json


def fully_parse_json(input):
    """
    Recursively parses a JSON string and all nested JSON strings inside it.
    Returns a fully decoded dict/list.
    """

    def decode(value):
        # If it's a string, try to parse it
        if isinstance(value, str):
            try:
                parsed = json.loads(value)
                # Recursively decode the parsed value
                return decode(parsed)
            except json.JSONDecodeError:
                return value
        # If it's a dict, decode each key/value
        elif isinstance(value, dict):
            return {k: decode(v) for k, v in value.items()}
        # If it's a list, decode each item
        elif isinstance(value, list):
            return [decode(v) for v in value]
        else:
            return value

    # Strip outer quotes if present
    input = input.strip()
    while input.startswith('"') and input.endswith('"'):
        input = input[1:-1]
        # Replace escaped quotes and backslashes
        input = input.replace('\\"', '"').replace('\\\\', '\\')

    # Parse outermost JSON
    try:
        outer = json.loads(input)
    except json.JSONDecodeError:
        # Not valid JSON
        return input

    return decode(outer)


class TracesHandler(BaseHandler):
    def __init__(self, logger):
        super().__init__(None, logger)

    async def get_traces(self, traces_request: GetTracesRequest) -> GetTracesResponse:
        """Get traces for a specific agent from LangTrace"""
        try:
            # Step 1: Get agent's API key and project ID from LangTrace
            url = f"{settings.LANGTRACE_BASE_URL}/api/v1/agents"
            agent_params = {"agent_name": traces_request.agent_name}

            self.logger.info(f"Retrieving agent info for: {traces_request.agent_name}")
            agent_response = requests.get(url, params=agent_params, timeout=30)

            if agent_response.status_code == 400:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Agent '{traces_request.agent_name}' not found in LangTrace"
                )
            elif agent_response.status_code != 200:
                raise HTTPException(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    detail=f"Failed to retrieve agent info from LangTrace: {agent_response.status_code}"
                )

            agent_data = agent_response.json()
            api_key = agent_data.get("api_key")
            project_id = agent_data.get("project_id")

            if not api_key or not project_id:
                raise HTTPException(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    detail="Invalid agent data received from LangTrace"
                )

            # Step 2: Forward request to LangTrace get-traces endpoint
            traces_url = f"{settings.LANGTRACE_BASE_URL}/api/v1/traces"
            headers = {
                "x-api-key": api_key,
                "Content-Type": "application/json"
            }

            payload = {
                "projectId": project_id,
                "pageSize": traces_request.page_size,
                "page": traces_request.page
            }

            self.logger.info(f"Fetching traces for agent: {traces_request.agent_name}, project: {project_id}")
            traces_response = requests.post(traces_url, headers=headers, json=payload, timeout=30)

            if traces_response.status_code != 200:
                raise HTTPException(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    detail=f"Failed to fetch traces from LangTrace: {traces_response.status_code} - {traces_response.text}"
                )

            # Step 3: Process and escape JSON strings in trace data
            traces_data = traces_response.json()

            # Log the full response from LangTrace for debugging
            self.logger.info(f"LangTrace API response: {traces_data}")

            # Process each trace node to escape JSON string fields
            if 'traces' in traces_data:
                self.logger.info(f"Processing {len(traces_data['traces'])} trace nodes")
                try:
                    traces_data['traces'] = self._process_trace_nodes(traces_data['traces'])
                    self.logger.debug("Trace nodes processing completed")
                except Exception as e:
                    self.logger.error(f"Error processing trace nodes, returning raw traces: {str(e)}")
                    # Keep the original traces data if processing fails

            return GetTracesResponse(**traces_data)
            
        except requests.exceptions.RequestException as e:
            self.logger.error(f"Network error while fetching traces: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_502_BAD_GATEWAY,
                detail=f"Network error connecting to LangTrace: {str(e)}"
            )
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"Unexpected error in get_traces: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Internal server error: {str(e)}"
            )

    def _process_trace_nodes(self, trace_nodes):
        """Recursively process trace nodes to escape JSON string fields"""
        processed_nodes = []

        for node in trace_nodes:
            try:
                processed_node = node.copy()

                # Process the trace data if it exists
                if 'trace' in processed_node:
                    try:
                        processed_node['trace'] = self._escape_trace_json_fields(processed_node['trace'])
                    except Exception as e:
                        self.logger.warning(f"Error processing trace data for node, keeping original: {str(e)}")
                        # Keep original trace data if processing fails

                # Recursively process children
                if 'children' in processed_node and processed_node['children']:
                    try:
                        processed_node['children'] = self._process_trace_nodes(processed_node['children'])
                    except Exception as e:
                        self.logger.warning(f"Error processing children nodes, keeping original: {str(e)}")
                        # Keep original children data if processing fails

                processed_nodes.append(processed_node)
            except Exception as e:
                self.logger.warning(f"Error processing trace node, keeping original: {str(e)}")
                processed_nodes.append(node)  # Keep original node if processing fails

        return processed_nodes

    def _escape_trace_json_fields(self, trace_data):
        """Escape JSON string fields in trace data"""
        processed_trace = trace_data.copy()

        # Fields that contain JSON strings that need proper escaping
        # Also include trace_state as it might contain JSON-like data
        json_string_fields = ['attributes', 'events', 'links', 'trace_state']

        for field in json_string_fields:
            if field in processed_trace and processed_trace[field]:
                if isinstance(processed_trace[field], str):
                    try:
                        original = processed_trace[field]
                        parsed_dict = fully_parse_json(processed_trace[field])
                        # convert the dict to json object
                        processed_trace[field] = json.dumps(parsed_dict, separators=(',', ':'), ensure_ascii=False)
                        if original != processed_trace[field]:
                            self.logger.debug(f"Escaped {field}: {original[:100]}... -> {processed_trace[field][:100]}...")
                    except Exception as e:
                        self.logger.warning(f"Error processing field '{field}', keeping original value: {str(e)}")
                        # Keep original field value if processing fails

        return processed_trace