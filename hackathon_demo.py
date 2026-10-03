import os
from openai import OpenAI

# 1. Point the client directly to your local Nasiko LLM Router Proxy
client = OpenAI(
    base_url="http://localhost:8085/v1",
    api_key="nasiko-local-token"  # Nasiko validates and injects the actual API key server-side
)

def run_multi_step_agent(task_description: str):
    print(f"🚀 Starting task: {task_description}")
    
    # Step 1: Analytical Sub-task
    print("\n--- Step 1: Planning with Nasiko Router ---")
    plan_response = client.chat.completions.create(
        model="nasiko/router",
        messages=[
            {
                "role": "system",
                "content": "You are a specialized Planning Agent operating under Nasiko governance."
            },
            {
                "role": "user",
                "content": f"Create a concise 3-step action plan for: {task_description}"
            }
        ]
    )
    plan = plan_response.choices[0].message.content
    print(plan)
    
    # Step 2: Synthesis & Execution Sub-task
    print("\n--- Step 2: Synthesis & Audit via Nasiko Router ---")
    exec_response = client.chat.completions.create(
        model="nasiko/router",
        messages=[
            {
                "role": "system",
                "content": "You are a Synthesis & Quality Agent. Review the plan and give the final code/execution output."
            },
            {
                "role": "user",
                "content": f"Execute this plan:\n{plan}"
            }
        ]
    )
    final_output = exec_response.choices[0].message.content
    print(final_output)

if __name__ == "__main__":
    test_task = "Build a dynamic rate-limiting algorithm for AI agent tool calls."
    run_multi_step_agent(test_task)