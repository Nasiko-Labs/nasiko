# CrewDesk Integration

CrewDesk is a multi-agent workspace that orchestrates specialized AI agents
through a Manager/Orchestrator workflow.

This guide demonstrates how CrewDesk can use Nasiko as the control plane
for its agent execution layer.

## Architecture

```text
CrewDesk Web
    |
    v
CrewDesk API
    |
    v
Manager / Orchestrator
    |
    +---- Researcher
    |         |
    |         v
    |      Nasiko
    |         |
    |         v
    |       Anakin
    |
    +---- Planner
    |
    +---- Builder
    |
    +---- Reviewer
    |
    +---- QA
    |
    +---- Documentation
