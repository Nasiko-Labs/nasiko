#!/usr/bin/env python3
"""
CLI for AgentCard Generator Agent
"""

import argparse
import json
import logging
import sys
from pathlib import Path

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from agent import AgentCardGeneratorAgent

logger = logging.getLogger(__name__)


def main():
    parser = argparse.ArgumentParser(
        description="Generate A2A-compliant AgentCards by analyzing agent code"
    )
    parser.add_argument("agent_path", help="Path to the agent directory to analyze")
    parser.add_argument(
        "-o",
        "--output",
        help="Output path for AgentCard.json (default: <agent_path>/AgentCard.json)",
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Show detailed progress"
    )
    parser.add_argument(
        "--model",
        default="gpt-4o",
        help="Model to use (default: gpt-4o)",
    )
    parser.add_argument(
        "--api-key", help="OpenAI API key (or set OPENAI_API_KEY env var)"
    )
    parser.add_argument(
        "--n8n-agent",
        action="store_true",
        help="Generate agent card from n8n workflow (expects n8n_workflow.json in agent_path)",
    )

    args = parser.parse_args()

    # Configure logging
    log_level = logging.DEBUG if args.verbose else logging.INFO
    logging.basicConfig(
        level=log_level,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    # Validate agent path
    agent_path = Path(args.agent_path)
    if not agent_path.exists():
        logger.error(f"Agent path '{args.agent_path}' does not exist")
        sys.exit(1)

    if not agent_path.is_dir():
        logger.error(f"'{args.agent_path}' is not a directory")
        sys.exit(1)

    output_path = args.output
    if not output_path:
        output_path = str(agent_path / "AgentCard.json")

    logger.info(f"Analyzing agent at: {args.agent_path}")
    logger.info(f"Output will be saved to: {output_path}")

    try:
        # Validate n8n workflow file if n8n_agent flag is set
        if args.n8n_agent:
            n8n_workflow_path = agent_path / "n8n_workflow.json"
            if not n8n_workflow_path.exists():
                logger.error(
                    f"n8n workflow file not found at '{n8n_workflow_path}'. "
                    "Expected n8n_workflow.json in the agent directory."
                )
                sys.exit(1)
            logger.info(f"Using n8n workflow file: {n8n_workflow_path}")

        logger.info(f"Initializing AgentCard Generator with model: {args.model}")
        agent = AgentCardGeneratorAgent(
            api_key=args.api_key, model=args.model, n8n_agent=args.n8n_agent
        )

        logger.info("Starting AgentCard generation")
        result = agent.generate_agentcard(
            agent_path=str(agent_path), verbose=args.verbose
        )

        if result["status"] == "success":
            agentcard = result.get("agentcard")

            if agentcard:
                logger.info(f"Saving AgentCard to: {output_path}")
                with open(output_path, "w", encoding="utf-8") as f:
                    json.dump(agentcard, f, indent=2)

                logger.info(
                    f"AgentCard generated successfully in {result.get('iterations', 'N/A')} iterations"
                )
                logger.info(
                    f"AgentCard preview - Name: {agentcard.get('name')}, Skills: {len(agentcard.get('skills', []))}"
                )
            else:
                logger.warning(
                    "AgentCard was not generated - the agent may not have created the final AgentCard JSON"
                )
                sys.exit(1)
        else:
            logger.error(f"AgentCard generation failed: {result['message']}")
            sys.exit(1)

    except ValueError as e:
        logger.error(
            f"Configuration error: {e} - Make sure to set OPENAI_API_KEY environment variable"
        )
        sys.exit(1)
    except Exception as e:
        logger.exception(f"Unexpected error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
