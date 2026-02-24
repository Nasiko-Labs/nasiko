import logging
from typing import Any

from pydantic import BaseModel

from policy_agent import PolicyAgent

logger = logging.getLogger(__name__)


class ComplianceCheckResponse(BaseModel):
    """Response model for compliance check"""

    status: str
    response: str
    error_message: str | None = None


class ComplianceToolset:
    """Compliance checking toolset for policy analysis"""

    def __init__(self, mongo_url: str, db_name: str):
        self.agent = PolicyAgent(mongo_url=mongo_url, db_name=db_name)
        self.session_id = "a2a_session"
        logger.info(f"Initialized ComplianceToolset with DB={db_name}")

    def check_compliance(
        self, document_text: str, query: str | None = None
    ) -> ComplianceCheckResponse:
        """Check document for policy compliance

        Args:
            document_text: The document text to analyze for compliance
            query: Optional specific question about compliance (default: "Analyze this document for policy compliance")

        Returns:
            ComplianceCheckResponse: Contains status and compliance analysis
        """
        if query is None:
            query = "Analyze this document for policy compliance"

        try:
            # Set the document text in the parser
            self.agent.document_parser.document_text = document_text
            logger.info(f"Checking compliance for document of length {len(document_text)}")

            # Get response from policy agent
            response = self.agent.get_response(query, session_id=self.session_id)

            return ComplianceCheckResponse(
                status="success",
                response=response,
            )
        except Exception as e:
            logger.error(f"Error checking compliance: {e}")
            return ComplianceCheckResponse(
                status="error",
                response="",
                error_message=f"Error checking compliance: {str(e)}",
            )

    def analyze_policy(self, policy_question: str) -> ComplianceCheckResponse:
        """Answer questions about policies

        Args:
            policy_question: Question about specific policies or compliance requirements

        Returns:
            ComplianceCheckResponse: Contains status and policy explanation
        """
        try:
            logger.info(f"Analyzing policy question: {policy_question}")

            # Get response from policy agent
            response = self.agent.get_response(
                policy_question, session_id=self.session_id
            )

            return ComplianceCheckResponse(
                status="success",
                response=response,
            )
        except Exception as e:
            logger.error(f"Error analyzing policy: {e}")
            return ComplianceCheckResponse(
                status="error",
                response="",
                error_message=f"Error analyzing policy: {str(e)}",
            )

    def get_tools(self) -> dict[str, Any]:
        """Return dictionary of available tools for OpenAI function calling"""
        return {
            "check_compliance": self,
            "analyze_policy": self,
        }