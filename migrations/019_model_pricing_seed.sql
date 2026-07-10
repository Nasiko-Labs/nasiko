-- Seed pricing for models in active use that were missing from the 0001 seed.
-- model_pricing is the single source of truth for cost calculation; the
-- static table in oss/observability/src/pricing.rs is only a fallback.

INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m, notes) VALUES
('openai', 'gpt-4.1', 2.00, 8.00, 'GPT-4.1'),
('openai', 'gpt-4.1-mini', 0.40, 1.60, 'GPT-4.1 mini'),
('openai', 'gpt-4.1-nano', 0.10, 0.40, 'GPT-4.1 nano'),
('openai', 'gpt-4-turbo', 10.00, 30.00, 'GPT-4 Turbo'),
('openai', 'gpt-3.5-turbo', 0.50, 1.50, 'GPT-3.5 Turbo'),
('openai', 'o3', 10.00, 40.00, 'o3'),
('openai', 'o3-mini', 1.10, 4.40, 'o3 mini'),
('anthropic', 'claude-3-5-sonnet', 3.00, 15.00, 'Claude 3.5 Sonnet'),
('anthropic', 'claude-3-5-haiku', 0.80, 4.00, 'Claude 3.5 Haiku'),
('google', 'gemini-2.5-pro', 1.25, 10.00, 'Gemini 2.5 Pro'),
('google', 'gemini-2.5-flash', 0.15, 0.60, 'Gemini 2.5 Flash'),
('deepseek', 'deepseek-v4-flash', 0.14, 0.28, 'DeepSeek V4 Flash')
ON CONFLICT (provider, model, effective_from) DO NOTHING;
