model_list:
  - model_name: platform-default-model
    litellm_params:
      model: "gpt-4o-mini"
      api_key: "os.environ/OPENAI_API_KEY"

# LiteLLM native callbacks hook directly into Arize Phoenix / OpenTelemetry
# maintaining the observable traces for the agent spans natively.
litellm_settings:
  success_callback: ["arize_phoenix"]
  failure_callback: ["arize_phoenix"]
