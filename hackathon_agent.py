
import time
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:8085/v1",
    api_key= "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"
)

prompts = [
    "Design a schema validator for AI agent outputs.",
    "Calculate optimal model routing weights based on token pricing.",
    "Analyze latency bottlenecks in multi-agent orchestration pipelines.",
    "Generate an automated circuit-breaker strategy for runaway agent loops."
]

for idx, prompt in enumerate(prompts, start=1):
    print(f"\n[Request {idx}/{len(prompts)}] Sending prompt: {prompt}")
    try:
        response = client.chat.completions.create(
            model="nasiko/router",
            messages=[
                {"role": "system", "content": "You are a backend engineering agent running under Nasiko."},
                {"role": "user", "content": prompt}
            ]
        )
        print("Response received:")
        print(response.choices[0].message.content[:150] + "...\n")
    except Exception as e:
        print(f"Error on request {idx}: {e}")
    time.sleep(2)

print("All requests completed. Trace materializer will process these within ~120s.")