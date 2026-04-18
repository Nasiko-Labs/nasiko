import os
from openai import OpenAI

# Use gateway instead of hardcoded API key
client = OpenAI(
    api_key=os.environ.get("LLM_VIRTUAL_KEY", "sk-nasiko-gateway-key"),
    base_url=os.environ.get("LLM_GATEWAY_URL", "http://nasiko-llm-gateway:4000"),
)

def chat(message: str) -> str:
    response = client.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": message}],
    )
    return response.choices[0].message.content

if __name__ == "__main__":
    print(chat("Hello! Are you working through the gateway?"))
