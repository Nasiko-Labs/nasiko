import logging
from typing import List, Dict

log = logging.getLogger(__name__)


def truncate_agent_cards(agent_cards: List[Dict[str, str]]) -> List[Dict[str, str]]:
    """
    Truncate agent cards to remove unnecessary fields for routing.

    Args:
        agent_cards: List of agent card dictionaries

    Returns:
        List of truncated agent card dictionaries
    """
    truncated_agent_cards = []

    for agent_card in agent_cards:
        try:
            name = agent_card.get("name")
            description = agent_card.get("description")

            if not name or not description:
                log.warning(
                    f"Agent card missing name or description, skipping: {agent_card}"
                )
                continue

            skills = []
            agent_skills = agent_card.get("skills", [])

            if not isinstance(agent_skills, list):
                log.warning(
                    f"Agent card skills is not a list for {name}, using empty list"
                )
                agent_skills = []

            for skill in agent_skills:
                if not isinstance(skill, dict):
                    log.warning(
                        f"Invalid skill format for agent {name}, skipping skill"
                    )
                    continue

                # Create a copy to avoid modifying original
                skill_copy = skill.copy()

                # Remove inputModes and outputModes if they exist
                skill_copy.pop("inputModes", None)
                skill_copy.pop("outputModes", None)

                skills.append(skill_copy)

            truncated_agent_cards.append(
                {"name": name, "description": description, "skills": skills}
            )
        except Exception as e:
            log.error(f"Error processing agent card: {e}, agent_card: {agent_card}")
            continue

    return truncated_agent_cards
