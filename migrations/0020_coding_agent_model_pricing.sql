INSERT INTO model_pricing
    (provider, model, input_price_per_1m, output_price_per_1m,
     cache_creation_price_per_1m, cache_read_price_per_1m, notes)
VALUES
    ('anthropic', 'claude-opus-5', 15.00, 75.00, 18.75, 1.50,
     'Claude Opus 5 - rate carried forward from Opus 4, verify'),
    ('anthropic', 'claude-sonnet-5', 3.00, 15.00, 3.75, 0.30,
     'Claude Sonnet 5 - rate carried forward from Sonnet 4, verify'),
    ('amazon-bedrock', 'openai.gpt-5.6-sol', 4.40, 22.00, 5.50, 0.44,
     'GPT-5.6 Sol via Amazon Bedrock Mantle, standard context')
ON CONFLICT DO NOTHING;
