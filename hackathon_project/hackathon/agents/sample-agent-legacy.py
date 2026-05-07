# demo_legacy_agent.py

class LegacyAgent:
    def __init__(self):
        self.api_key = "sk-openai-real-key"

        print("Legacy Agent Initialized")
        print(f"Provider: OpenAI")
        print(f"API Key: {self.api_key[:10]}...")

    def process(self, message):
        print("\n--- DIRECT CALL ---")
        print("Calling OpenAI directly")
        print(f"Message: {message}")

        return "OpenAI Response: Basic answer."


if __name__ == "__main__":
    agent = LegacyAgent()

    res = agent.process("Explain LLM gateways in one line")
    print("\nResponse:", res)