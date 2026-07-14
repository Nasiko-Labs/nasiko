-- Seed pricing for models in active use. model_pricing is the single source
-- of truth for cost calculation; the static table in
-- oss/observability/src/pricing.rs is only a fallback.

INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m, cache_creation_price_per_1m, cache_read_price_per_1m, notes) VALUES
('openai', 'gpt-4o', 2.50, 10.00, NULL, NULL, 'GPT-4o standard'),
('openai', 'gpt-4o-mini', 0.15, 0.60, NULL, NULL, 'GPT-4o mini'),
('openai', 'gpt-4.1', 2.00, 8.00, NULL, NULL, 'GPT-4.1'),
('openai', 'gpt-4.1-mini', 0.40, 1.60, NULL, NULL, 'GPT-4.1 mini'),
('openai', 'gpt-4.1-nano', 0.10, 0.40, NULL, NULL, 'GPT-4.1 nano'),
('openai', 'gpt-4-turbo', 10.00, 30.00, NULL, NULL, 'GPT-4 Turbo'),
('openai', 'gpt-3.5-turbo', 0.50, 1.50, NULL, NULL, 'GPT-3.5 Turbo'),
('openai', 'o1-preview', 15.00, 60.00, NULL, NULL, 'o1 preview'),
('openai', 'o1-mini', 3.00, 12.00, NULL, NULL, 'o1 mini'),
('openai', 'o3', 10.00, 40.00, NULL, NULL, 'o3'),
('openai', 'o3-mini', 1.10, 4.40, NULL, NULL, 'o3 mini'),
('anthropic', 'claude-opus-4', 15.00, 75.00, 18.75, 1.50, 'Claude Opus 4'),
('anthropic', 'claude-sonnet-4', 3.00, 15.00, 3.75, 0.30, 'Claude Sonnet 4'),
('anthropic', 'claude-haiku-4', 0.80, 4.00, 1.00, 0.08, 'Claude Haiku 4'),
('anthropic', 'claude-3-5-sonnet', 3.00, 15.00, NULL, NULL, 'Claude 3.5 Sonnet'),
('anthropic', 'claude-3-5-haiku', 0.80, 4.00, NULL, NULL, 'Claude 3.5 Haiku'),
('google', 'gemini-2.5-pro', 1.25, 10.00, NULL, NULL, 'Gemini 2.5 Pro'),
('google', 'gemini-2.5-flash', 0.15, 0.60, NULL, NULL, 'Gemini 2.5 Flash'),
('groq', 'llama-3.3-70b-versatile', 0.59, 0.79, NULL, NULL, 'Llama 3.3 70B on Groq'),
('groq', 'llama-3.1-8b-instant', 0.05, 0.08, NULL, NULL, 'Llama 3.1 8B on Groq'),
('deepseek', 'deepseek-chat', 0.14, 0.28, 0.014, 0.014, 'DeepSeek Chat'),
('deepseek', 'deepseek-reasoner', 0.55, 2.19, NULL, NULL, 'DeepSeek R1'),
('deepseek', 'deepseek-v4-flash', 0.14, 0.28, NULL, NULL, 'DeepSeek V4 Flash');
