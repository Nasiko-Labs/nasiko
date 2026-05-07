# demo_gateway_agent.py

class DemoGatewayAgent:
    def __init__(self):
        self.base_url = "http://localhost:4000"
        self.virtual_key = "sk-litellm-demo123"

        print("Gateway Agent Initialized")
        print(f"Gateway URL: {self.base_url}")
        print(f"Virtual Key: {self.virtual_key[:10]}...")

    def process(self, message, provider="gemini"):
        print("\n--- REQUEST ---")
        print(f"Routing via Gateway → Provider: {provider}")
        print(f"Message: {message}")

        # Simulated response (NO API CALL)
        if provider == "gemini":
            return "Gemini Response: Fast and cost-efficient answer."
        elif provider == "gpt-4o":
            return "GPT-4o Response: More detailed and refined answer."
        else:
            return "Claude Response: Long context optimized answer."


if __name__ == "__main__":
    agent = DemoGatewayAgent()

    res = agent.process("Explain LLM gateways in one line", provider="gemini")
    print("\nResponse:", res)