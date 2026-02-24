from fastapi import HTTPException, status
from app.pkg.config.config import settings
import requests
import json
from typing import Dict, Any, List
import re


class ObservabilityService:
    def __init__(self, logger):
        self.logger = logger
    
    def _camel_to_snake(self, name: str) -> str:
        """Convert camelCase to snake_case"""
        s1 = re.sub('(.)([A-Z][a-z]+)', r'\1_\2', name)
        return re.sub('([a-z0-9])([A-Z])', r'\1_\2', s1).lower()
    
    def _convert_keys_to_snake_case(self, data: Any) -> Any:
        """Recursively convert all dictionary keys from camelCase to snake_case"""
        if isinstance(data, dict):
            return {self._camel_to_snake(k): self._convert_keys_to_snake_case(v) for k, v in data.items()}
        elif isinstance(data, list):
            return [self._convert_keys_to_snake_case(item) for item in data]
        else:
            return data

# Removed Pydantic conversion methods since we now return dictionaries directly

    async def get_all_sessions(self, user_id: str, auth_header: str, start_time: str = None) -> Dict[str, Any]:
        """Get sessions from all agents/projects the user has access to"""
        try:
            from app.pkg.auth import AuthClient
            
            # Step 1: Get all user-accessible agent IDs using existing auth functions
            auth_client = AuthClient()
            accessible_agent_ids = await auth_client.get_user_accessible_agents(auth_header)
            
            if not accessible_agent_ids:
                return {
                    "data": {
                        "sessions": [],
                        "total_agents": 0,
                        "pagination": {"end_cursor": None, "has_next_page": False}
                    }
                }
            
            self.logger.info(f"Found {len(accessible_agent_ids)} accessible agents for user {user_id}")
            
            # Step 2: Get sessions from each agent's project
            all_sessions = []
            successful_agents = 0
            
            for agent_id in accessible_agent_ids:
                try:
                    # Use agent_id as project_name to get Phoenix project ID
                    project_id = await self._get_project_id(agent_id)
                    
                    if project_id:
                        project_sessions = await self._get_project_sessions_for_aggregation(project_id, agent_id, start_time)
                        all_sessions.extend(project_sessions)
                        successful_agents += 1
                except Exception as e:
                    self.logger.warning(f"Failed to get sessions for agent {agent_id}: {e}")
                    continue
            
            # Step 3: Sort sessions by start time (most recent first)
            all_sessions.sort(key=lambda x: x.get('start_time', ''), reverse=True)
            
            return {
                "data": {
                    "sessions": all_sessions,
                    "total_agents": len(accessible_agent_ids),
                    "successful_agents": successful_agents,
                    "pagination": {"end_cursor": None, "has_next_page": False}
                }
            }
            
        except Exception as e:
            self.logger.error(f"Error getting all sessions: {e}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to retrieve sessions: {str(e)}"
            )

    async def _get_project_sessions_for_aggregation(self, project_id: str, agent_id: str, start_time: str = None) -> List[Dict[str, Any]]:
        """Get sessions for a specific project using the GraphQL query for aggregation"""
        try:
            # Build time range for query - use provided start_time or default to last 7 days
            if start_time:
                query_time_range = {"start": start_time}
            else:
                # Default to last 7 days if no start_time provided
                from datetime import datetime, timedelta
                default_start = datetime.utcnow() - timedelta(days=7)
                query_time_range = {"start": default_start.isoformat() + "Z"}
            
            query = """
            query ProjectPageQueriesSessionsQuery($id: ID!, $timeRange: TimeRange!) {
                project: node(id: $id) {
                    __typename
                    ... on Project {
                        name
                        sessions(first: 30, sort: {col: startTime, dir: desc}, timeRange: $timeRange) {
                            edges {
                                session: node {
                                    id
                                    sessionId
                                    numTraces
                                    startTime
                                    endTime
                                    firstInput {
                                        value
                                    }
                                    lastOutput {
                                        value
                                    }
                                    tokenUsage {
                                        total
                                    }
                                    traceLatencyMsP50: traceLatencyMsQuantile(probability: 0.5)
                                    traceLatencyMsP99: traceLatencyMsQuantile(probability: 0.99)
                                    costSummary {
                                        total {
                                            cost
                                        }
                                    }
                                    sessionAnnotations {
                                        id
                                        name
                                        label
                                        score
                                        annotatorKind
                                        user {
                                            username
                                            profilePictureUrl
                                            id
                                        }
                                    }
                                    sessionAnnotationSummaries {
                                        labelFractions {
                                            fraction
                                            label
                                        }
                                        meanScore
                                        name
                                    }
                                }
                                cursor
                            }
                            pageInfo {
                                endCursor
                                hasNextPage
                            }
                        }
                    }
                    id
                }
            }
            """
            
            variables = {
                "id": project_id,
                "timeRange": query_time_range
            }
            
            raw_response = await self._execute_graphql_query(query, variables)
            
            # Transform response to snake_case
            project_data = raw_response.get("data", {}).get("project", {})
            sessions_data = project_data.get("sessions", {}).get("edges", [])
            
            sessions = []
            for edge in sessions_data:
                session_data = edge.get("session", {})
                if session_data:
                    # Convert to snake_case and add agent_id for identification
                    session_snake = self._convert_keys_to_snake_case(session_data)
                    session_snake["agent_id"] = agent_id
                    sessions.append(session_snake)
            
            self.logger.info(f"Retrieved {len(sessions)} sessions for agent {agent_id}")
            return sessions
            
        except Exception as e:
            self.logger.error(f"Error getting sessions for agent {agent_id}: {e}")
            return []

    async def get_session_details(self, session_id: str) -> Dict[str, Any]:
        """Get session details from observability service and return transformed response"""
        try:
            # Step 1: Get session ID by session string
            session_node_id = await self._get_session_node_id(session_id)
            
            # Step 2: Get session details
            raw_session_details = await self._get_session_details_by_id(session_node_id)
            
            # Transform session response for better client consumption
            return self._transform_session_response(raw_session_details)
            
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"Unexpected error in get_session_details: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Internal server error: {str(e)}"
            )

    async def get_trace_details(self, trace_id: str, project_id: str) -> Dict[str, Any]:
        """Get trace details from observability service and return nested span structure"""
        try:
            # Get trace details
            query = """
            query TraceDetailsQuery(
              $traceId: ID!
              $id: ID!
            ) {
              project: node(id: $id) {
                __typename
                ... on Project {
                  trace(traceId: $traceId) {
                    projectSessionId
                    ...ConnectedTraceTree
                    rootSpans: spans(first: 1, rootSpansOnly: true, orphanSpanAsRootSpan: true) {
                      edges {
                        span: node {
                          statusCode
                          id
                          spanId
                          parentId
                        }
                      }
                    }
                    latencyMs
                    costSummary {
                      prompt {
                        cost
                      }
                      completion {
                        cost
                      }
                      total {
                        cost
                      }
                    }
                    id
                  }
                }
                id
              }
            }

            fragment ConnectedTraceTree on Trace {
              numSpans
              spans(first: 1000) {
                edges {
                  span: node {
                    id
                    spanId
                    name
                    spanKind
                    statusCode
                    startTime
                    endTime
                    parentId
                    latencyMs
                    tokenCountTotal
                    spanAnnotationSummaries {
                      labels
                      count
                      labelCount
                      labelFractions {
                        fraction
                        label
                      }
                      name
                      scoreCount
                      meanScore
                    }
                  }
                  cursor
                  node {
                    __typename
                    id
                  }
                }
                pageInfo {
                  endCursor
                  hasNextPage
                }
              }
              id
            }
            """
            
            variables = {
                "traceId": trace_id,
                "id": project_id
            }
            
            raw_response = await self._execute_graphql_query(query, variables)
            
            # Transform the response to have nested spans structure
            return self._transform_trace_response(raw_response)
            
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"Unexpected error in get_trace_details: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Internal server error: {str(e)}"
            )

    async def get_span_details(self, span_id: str) -> Dict[str, Any]:
        """Get span details from observability service with parsed JSON attributes"""
        try:
            query = """
            query SpanDetailsQuery(
              $id: ID!
            ) {
              span: node(id: $id) {
                __typename
                ... on Span {
                  id
                  spanId
                  trace {
                    id
                    traceId
                  }
                  name
                  spanKind
                  statusCode: propagatedStatusCode
                  statusMessage
                  startTime
                  parentId
                  latencyMs
                  tokenCountTotal
                  endTime
                  input {
                    value
                    mimeType
                  }
                  output {
                    value
                    mimeType
                  }
                  attributes
                  events {
                    name
                    message
                    timestamp
                  }
                  documentRetrievalMetrics {
                    evaluationName
                    ndcg
                    precision
                    hit
                  }
                  documentEvaluations {
                    documentPosition
                    name
                    label
                    score
                    explanation
                    id
                  }
                  spanAnnotations {
                    id
                    name
                  }
                  ...SpanHeader_span
                  ...SpanFeedback_annotations
                  ...SpanAside_span
                }
                id
              }
            }

            fragment AnnotationConfigListProjectAnnotationConfigFragment on Project {
              annotationConfigs {
                edges {
                  node {
                    __typename
                    ... on Node {
                      __isNode: __typename
                      id
                    }
                    ... on AnnotationConfigBase {
                      __isAnnotationConfigBase: __typename
                      name
                      annotationType
                      description
                    }
                    ... on CategoricalAnnotationConfig {
                      values {
                        label
                        score
                      }
                    }
                    ... on ContinuousAnnotationConfig {
                      lowerBound
                      upperBound
                      optimizationDirection
                    }
                    ... on FreeformAnnotationConfig {
                      name
                    }
                  }
                }
              }
            }

            fragment AnnotationSummaryGroup on Span {
              project {
                id
                annotationConfigs {
                  edges {
                    node {
                      __typename
                      ... on AnnotationConfigBase {
                        __isAnnotationConfigBase: __typename
                        annotationType
                      }
                      ... on CategoricalAnnotationConfig {
                        id
                        name
                        optimizationDirection
                        values {
                          label
                          score
                        }
                      }
                      ... on Node {
                        __isNode: __typename
                        id
                      }
                    }
                  }
                }
              }
              spanAnnotations {
                id
                name
                label
                score
                annotatorKind
                createdAt
                user {
                  username
                  profilePictureUrl
                  id
                }
              }
              spanAnnotationSummaries {
                labelFractions {
                  fraction
                  label
                }
                meanScore
                name
              }
            }

            fragment SpanAsideAnnotationList_span on Span {
              project {
                id
                annotationConfigs {
                  configs: edges {
                    config: node {
                      __typename
                      ... on Node {
                        __isNode: __typename
                        id
                      }
                      ... on AnnotationConfigBase {
                        __isAnnotationConfigBase: __typename
                        name
                      }
                    }
                  }
                }
              }
              spanAnnotations {
                id
              }
              ...AnnotationSummaryGroup
            }

            fragment SpanAside_span on Span {
              id
              project {
                id
                ...AnnotationConfigListProjectAnnotationConfigFragment
                annotationConfigs {
                  configs: edges {
                    config: node {
                      __typename
                      ... on Node {
                        __isNode: __typename
                        id
                      }
                      ... on AnnotationConfigBase {
                        __isAnnotationConfigBase: __typename
                        name
                        description
                        annotationType
                      }
                      ... on CategoricalAnnotationConfig {
                        values {
                          label
                          score
                        }
                      }
                      ... on ContinuousAnnotationConfig {
                        lowerBound
                        upperBound
                        optimizationDirection
                      }
                      ... on FreeformAnnotationConfig {
                        name
                      }
                    }
                  }
                }
              }
              code: statusCode
              startTime
              endTime
              tokenCountTotal
              ...TraceHeaderRootSpanAnnotationsFragment
              ...SpanAsideAnnotationList_span
              ...AnnotationSummaryGroup
            }

            fragment SpanFeedback_annotations on Span {
              id
              spanAnnotations {
                id
                name
                label
                score
                explanation
                metadata
                annotatorKind
                identifier
                source
                createdAt
                updatedAt
                user {
                  id
                  username
                  profilePictureUrl
                }
              }
            }

            fragment SpanHeader_span on Span {
              id
              name
              spanKind
              code: statusCode
              latencyMs
              startTime
              tokenCountTotal
              costSummary {
                total {
                  cost
                }
              }
            }

            fragment TraceHeaderRootSpanAnnotationsFragment on Span {
              ...AnnotationSummaryGroup
            }
            """
            
            variables = {"id": span_id}
            
            raw_response = await self._execute_graphql_query(query, variables)
            
            # Transform span response for better client consumption
            return self._transform_span_response(raw_response)
            
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"Unexpected error in get_span_details: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Internal server error: {str(e)}"
            )

    async def _get_project_id(self, project_name: str) -> str:
        """Get project ID by project name"""
        url = f"{settings.PHOENIX_SERVICE_URL}/v1/projects/{project_name}"
        
        try:
            response = requests.get(url, timeout=30)
            
            if response.status_code == 404:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Project '{project_name}' not found"
                )
            elif response.status_code != 200:
                raise HTTPException(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    detail=f"Failed to get project ID: {response.status_code}"
                )
            
            data = response.json()
            return data["data"]["id"]
            
        except requests.exceptions.RequestException as e:
            self.logger.error(f"Network error while getting project ID: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_502_BAD_GATEWAY,
                detail=f"Network error connecting to Phoenix: {str(e)}"
            )

    async def _get_session_node_id(self, session_id: str) -> str:
        """Get session node ID by session string ID"""
        query = """
        query GetProjectSessionById($sessionId: String!) {
            getProjectSessionById(sessionId: $sessionId) {
              id
            }
          }
        """
        
        variables = {"sessionId": session_id}
        
        response = await self._execute_graphql_query(query, variables)
        
        if not response.get("data", {}).get("getProjectSessionById"):
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"Session '{session_id}' not found"
            )
        
        return response["data"]["getProjectSessionById"]["id"]

    async def _get_session_details_by_id(self, session_node_id: str) -> Dict[str, Any]:
        """Get session details by session node ID"""
        query = """
        query SessionDetailsQuery(
          $id: ID!
          $first: Int
        ) {
          session: node(id: $id) {
            __typename
            ... on ProjectSession {
              numTraces
              tokenUsage {
                total
              }
              costSummary {
                total {
                  cost
                  tokens
                }
                prompt {
                  cost
                  tokens
                }
                completion {
                  cost
                  tokens
                }
              }
              sessionId
              latencyP50: traceLatencyMsQuantile(probability: 0.5)
              ...SessionDetailsTraceList_traces_3ASum4
            }
            id
          }
        }

        fragment AnnotationSummaryGroup on Span {
          project {
            id
            annotationConfigs {
              edges {
                node {
                  __typename
                  ... on AnnotationConfigBase {
                    __isAnnotationConfigBase: __typename
                    annotationType
                  }
                  ... on CategoricalAnnotationConfig {
                    id
                    name
                    optimizationDirection
                    values {
                      label
                      score
                    }
                  }
                  ... on Node {
                    __isNode: __typename
                    id
                  }
                }
              }
            }
          }
          spanAnnotations {
            id
            name
            label
            score
            annotatorKind
            createdAt
            user {
              username
              profilePictureUrl
              id
            }
          }
          spanAnnotationSummaries {
            labelFractions {
              fraction
              label
            }
            meanScore
            name
          }
        }

        fragment SessionDetailsTraceList_traces_3ASum4 on ProjectSession {
          traces(first: $first) {
            edges {
              trace: node {
                id
                traceId
                rootSpan {
                  trace {
                    id
                    costSummary {
                      total {
                        cost
                      }
                    }
                  }
                  id
                  attributes
                  project {
                    id
                  }
                  input {
                    value
                    mimeType
                  }
                  output {
                    value
                    mimeType
                  }
                  cumulativeTokenCountTotal
                  latencyMs
                  startTime
                  spanId
                  ...AnnotationSummaryGroup
                }
              }
              cursor
              node {
                __typename
                id
              }
            }
            pageInfo {
              endCursor
              hasNextPage
            }
          }
          id
        }
        """
        
        variables = {
            "id": session_node_id,
            "first": 100  # Increased to ensure we get traces
        }
        
        return await self._execute_graphql_query(query, variables)

    async def _execute_graphql_query(self, query: str, variables: Dict[str, Any]) -> Dict[str, Any]:
        """Execute a GraphQL query against Phoenix service"""
        url = f"{settings.PHOENIX_SERVICE_URL}/graphql"
        
        payload = {
            "query": query,
            "variables": variables
        }
        
        headers = {
            "Content-Type": "application/json"
        }
        
        try:
            response = requests.post(url, json=payload, headers=headers, timeout=30)
            
            if response.status_code != 200:
                raise HTTPException(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    detail=f"GraphQL request failed: {response.status_code} - {response.text}"
                )
            
            return response.json()
            
        except requests.exceptions.RequestException as e:
            self.logger.error(f"Network error during GraphQL request: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_502_BAD_GATEWAY,
                detail=f"Network error connecting to Phoenix: {str(e)}"
            )

    def _transform_trace_response(self, raw_response: Dict[str, Any]) -> Dict[str, Any]:
        """Transform the flat spans structure into a nested tree structure"""
        try:
            # Extract the trace data
            trace_data = raw_response.get("data", {}).get("project", {}).get("trace", {})
            if not trace_data:
                return self._convert_keys_to_snake_case(raw_response)

            # Get flat list of spans
            span_edges = trace_data.get("spans", {}).get("edges", [])
            if not span_edges:
                return self._convert_keys_to_snake_case(raw_response)

            # Create a dictionary for quick span lookup by spanId
            spans_by_span_id = {}
            spans_by_id = {}
            
            # Process all spans and create clean span objects
            for edge in span_edges:
                span = edge.get("span", {})
                if span:
                    # Clean up the span data
                    cleaned_span = self._clean_span_data(span)
                    span_id = span.get("spanId")
                    node_id = span.get("id")
                    
                    if span_id:
                        spans_by_span_id[span_id] = cleaned_span
                    if node_id:
                        spans_by_id[node_id] = cleaned_span

            # Build the tree structure
            nested_spans = self._build_span_tree(spans_by_span_id)

            # Convert main trace data to snake_case
            cost_summary = self._convert_keys_to_snake_case(trace_data.get("costSummary", {}))
            root_spans = self._convert_keys_to_snake_case(trace_data.get("rootSpans", {}))

            # Return transformed response directly as dictionary with snake_case fields
            return {
                "data": {
                    "trace": {
                        "id": trace_data.get("id"),
                        "project_session_id": trace_data.get("projectSessionId"),
                        "num_spans": trace_data.get("numSpans", len(span_edges)),
                        "latency_ms": trace_data.get("latencyMs"),
                        "cost_summary": cost_summary,
                        "root_spans": root_spans,
                        "spans": nested_spans,  # This is now the nested structure with snake_case
                        "span_lookup": spans_by_id  # Keep flat lookup for span details access
                    },
                    "project_id": raw_response.get("data", {}).get("project", {}).get("id")
                }
            }

        except Exception as e:
            self.logger.warning(f"Failed to transform trace response: {str(e)}, returning raw response")
            return self._convert_keys_to_snake_case(raw_response)

    def _clean_span_data(self, span: Dict[str, Any]) -> Dict[str, Any]:
        """Clean and standardize span data with snake_case conversion"""
        cleaned = {
            "id": span.get("id"),
            "span_id": span.get("spanId"),
            "name": span.get("name"),
            "span_kind": span.get("spanKind"),
            "status_code": span.get("statusCode"),
            "start_time": span.get("startTime"),
            "end_time": span.get("endTime"),
            "parent_id": span.get("parentId"),
            "latency_ms": span.get("latencyMs"),
            "token_count_total": span.get("tokenCountTotal"),
            "span_annotation_summaries": self._convert_keys_to_snake_case(span.get("spanAnnotationSummaries", [])),
            "children": []  # Will be populated during tree building
        }
        return cleaned

    def _build_span_tree(self, spans_by_span_id: Dict[str, Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Build a nested tree structure from flat spans using parentId relationships"""
        root_spans = []
        
        # First pass: identify root spans (no parent_id) and attach children
        for span_id, span in spans_by_span_id.items():
            parent_id = span.get("parent_id")
            
            if parent_id is None:
                # This is a root span
                root_spans.append(span)
            else:
                # This span has a parent, add it to parent's children
                parent_span = spans_by_span_id.get(parent_id)
                if parent_span:
                    parent_span["children"].append(span)
                else:
                    # Parent not found, treat as root (orphaned span)
                    self.logger.warning(f"Orphaned span found: {span_id} with parent {parent_id}")
                    root_spans.append(span)

        # Sort spans by start time for better readability
        def sort_spans_recursive(spans):
            spans.sort(key=lambda s: s.get("start_time", ""))
            for span in spans:
                if span.get("children"):
                    sort_spans_recursive(span["children"])

        sort_spans_recursive(root_spans)
        return root_spans

    def _transform_session_response(self, raw_response: Dict[str, Any]) -> Dict[str, Any]:
        """Transform session response for better client consumption"""
        try:
            session_data = raw_response.get("data", {}).get("session", {})
            if not session_data:
                # If no session data, create empty response structure
                return {
                    "data": {
                        "session": {
                            "id": "",
                            "session_id": "",
                            "num_traces": 0,
                            "token_usage": {"total": 0.0},
                            "cost_summary": {},
                            "latency_p50": None,
                            "traces": [],
                            "pagination": {"end_cursor": None, "has_next_page": False}
                        }
                    }
                }

            # Extract and clean traces data from GraphQL connection format
            traces_connection = session_data.get("traces", {})
            traces_edges = traces_connection.get("edges", [])
            cleaned_traces = []
            
            self.logger.info(f"Processing {len(traces_edges)} trace edges")
            
            for edge in traces_edges:
                try:
                    trace = edge.get("trace", {})
                    if trace:
                        # Clean and prepare rootSpan data to match Pydantic model expectations
                        root_span = trace.get("rootSpan")
                        if root_span:
                            try:
                                # Convert root span while being selective about nested structures
                                cleaned_root_span = {
                                    "id": root_span.get("id"),
                                    "spanId": root_span.get("spanId"),
                                    "attributes": root_span.get("attributes"),
                                    "cumulativeTokenCountTotal": root_span.get("cumulativeTokenCountTotal"),
                                    "latencyMs": root_span.get("latencyMs"),
                                    "startTime": root_span.get("startTime"),
                                    "spanAnnotations": root_span.get("spanAnnotations", []),
                                    "spanAnnotationSummaries": root_span.get("spanAnnotationSummaries", [])
                                }
                                
                                # Handle project field carefully - model expects Dict[str, str]
                                project = root_span.get("project")
                                if project and isinstance(project, dict):
                                    cleaned_root_span["project"] = {
                                        "id": str(project.get("id", ""))
                                    }
                                
                                # Handle input/output fields - model expects Dict[str, str]
                                input_data = root_span.get("input")
                                if input_data and isinstance(input_data, dict):
                                    cleaned_root_span["input"] = {k: str(v) if v is not None else "" for k, v in input_data.items()}
                                
                                output_data = root_span.get("output")
                                if output_data and isinstance(output_data, dict):
                                    cleaned_root_span["output"] = {k: str(v) if v is not None else "" for k, v in output_data.items()}
                                
                                # Handle trace field
                                trace_data = root_span.get("trace")
                                if trace_data and isinstance(trace_data, dict):
                                    cleaned_trace_data = {
                                        "id": trace_data.get("id")
                                    }
                                    cost_summary = trace_data.get("costSummary")
                                    if cost_summary:
                                        cleaned_trace_data["costSummary"] = self._convert_keys_to_snake_case(cost_summary)
                                    cleaned_root_span["trace"] = cleaned_trace_data
                                
                                root_span = cleaned_root_span
                            except Exception as e:
                                self.logger.warning(f"Failed to clean rootSpan data: {e}")
                                # Fall back to basic conversion or original
                                root_span = trace.get("rootSpan")
                        
                        cleaned_trace = {
                            "id": trace.get("id"),
                            "traceId": trace.get("traceId"),  # Use alias name for Pydantic
                            "rootSpan": root_span,  # Use alias name for Pydantic
                            "cursor": edge.get("cursor")
                        }
                        cleaned_traces.append(cleaned_trace)
                        self.logger.debug(f"Successfully processed trace: {trace.get('id')}")
                    else:
                        self.logger.warning(f"Empty trace in edge: {edge}")
                except Exception as e:
                    self.logger.error(f"Error processing trace edge: {e}, edge: {edge}")
                    # Continue processing other traces instead of failing completely

            self.logger.info(f"Cleaned {len(cleaned_traces)} traces")

            # Convert field names to snake_case and clean traces
            token_usage = self._convert_keys_to_snake_case(session_data.get("tokenUsage", {}))
            cost_summary = self._convert_keys_to_snake_case(session_data.get("costSummary", {}))
            page_info = self._convert_keys_to_snake_case(traces_connection.get("pageInfo", {}))
            
            # Convert traces to snake_case as well
            cleaned_traces_snake = []
            for trace in cleaned_traces:
                trace_snake = self._convert_keys_to_snake_case(trace)
                cleaned_traces_snake.append(trace_snake)

            # Return transformed response directly as dictionary with snake_case fields
            return {
                "data": {
                    "session": {
                        "id": session_data.get("id"),
                        "session_id": session_data.get("sessionId"),
                        "num_traces": session_data.get("numTraces"),
                        "token_usage": token_usage,
                        "cost_summary": cost_summary,
                        "latency_p50": session_data.get("latencyP50"),
                        "traces": cleaned_traces_snake,
                        "pagination": page_info if page_info else {"end_cursor": None, "has_next_page": False}
                    }
                }
            }

        except Exception as e:
            self.logger.warning(f"Failed to transform session response: {str(e)}, returning fallback response")
            # If transformation fails, create a minimal valid response structure
            fallback_response = {
                "data": {
                    "session": {
                        "id": session_data.get("id", "") if 'session_data' in locals() and session_data else "",
                        "session_id": session_data.get("sessionId", "") if 'session_data' in locals() and session_data else "",
                        "num_traces": session_data.get("numTraces", 0) if 'session_data' in locals() and session_data else 0,
                        "token_usage": session_data.get("tokenUsage", {"total": 0.0}) if 'session_data' in locals() and session_data else {"total": 0.0},
                        "cost_summary": session_data.get("costSummary", {}) if 'session_data' in locals() and session_data else {},
                        "latency_p50": session_data.get("latencyP50", None) if 'session_data' in locals() and session_data else None,
                        "traces": [],  # Empty list instead of connection object
                        "pagination": {"end_cursor": None, "has_next_page": False}
                    }
                }
            }
            return fallback_response

    async def get_agent_project_stats(self, agent_id: str, start_time: str) -> Dict[str, Any]:
        """Get project statistics for an agent from observability service"""
        try:
            # Step 1: Get project ID from agent ID
            project_id = await self._get_project_id(agent_id)
            
            if not project_id:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Project not found for agent '{agent_id}'"
                )
            
            # Step 2: Execute the project stats GraphQL query
            query = """
            query ProjectPageQuery(
              $id: ID!
              $timeRange: TimeRange!
            ) {
              project: node(id: $id) {
                __typename
                ...ProjectPageHeader_stats
                ...StreamToggle_data
                id
              }
            }

            fragment ProjectPageHeader_stats on Project {
              traceCount(timeRange: $timeRange)
              costSummary(timeRange: $timeRange) {
                total {
                  cost
                }
                prompt {
                  cost
                }
                completion {
                  cost
                }
              }
              latencyMsP50: latencyMsQuantile(probability: 0.5, timeRange: $timeRange)
              latencyMsP99: latencyMsQuantile(probability: 0.99, timeRange: $timeRange)
              spanAnnotationNames
              documentEvaluationNames
              id
            }

            fragment StreamToggle_data on Project {
              streamingLastUpdatedAt
              id
            }
            """
            
            variables = {
                "id": project_id,
                "timeRange": {
                    "start": start_time
                }
            }
            
            raw_response = await self._execute_graphql_query(query, variables)
            
            # Step 3: Transform response to snake_case
            project_data = raw_response.get("data", {}).get("project", {})
            
            if not project_data:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Project data not found for agent '{agent_id}'"
                )
            
            # Convert to snake_case format for client consumption
            converted_data = self._convert_keys_to_snake_case(project_data)
            
            return {
                "data": {
                    "project": converted_data
                }
            }
            
        except HTTPException:
            raise
        except Exception as e:
            self.logger.error(f"Error getting agent project stats: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to retrieve project stats: {str(e)}"
            )

    def _transform_span_response(self, raw_response: Dict[str, Any]) -> Dict[str, Any]:
        """Transform span response for better client consumption with snake_case conversion"""
        try:
            span_data = raw_response.get("data", {}).get("span", {})
            if not span_data:
                return self._convert_keys_to_snake_case(raw_response)

            # Parse JSON strings in attributes if present
            attributes = span_data.get("attributes")
            if attributes and isinstance(attributes, str):
                try:
                    span_data["attributes"] = json.loads(attributes)
                except json.JSONDecodeError:
                    self.logger.warning("Failed to parse span attributes as JSON")

            # Parse input/output values if they're JSON strings
            input_data = span_data.get("input", {})
            if input_data and input_data.get("mimeType") == "json" and input_data.get("value"):
                try:
                    input_data["parsed_value"] = json.loads(input_data["value"])
                except json.JSONDecodeError:
                    pass

            output_data = span_data.get("output", {})
            if output_data and output_data.get("mimeType") == "json" and output_data.get("value"):
                try:
                    output_data["parsed_value"] = json.loads(output_data["value"])
                except json.JSONDecodeError:
                    pass

            # Convert entire span data to snake_case and return directly
            converted_span = self._convert_keys_to_snake_case(span_data)

            return {
                "data": {
                    "span": converted_span
                }
            }

        except Exception as e:
            self.logger.warning(f"Failed to transform span response: {str(e)}, returning raw response")
            return self._convert_keys_to_snake_case(raw_response)