-- Seed catalog rows so LLM Router UI lists Issue-4 providers (mistral / ollama / azure)
-- alongside the existing openai / anthropic / gemini / groq pricing data.
-- Prices are best-effort public list rates — verify before relying on cost figures.
-- Ollama is local (zero cloud cost). Azure rows use deployment-name placeholders.

INSERT INTO model_pricing
    (provider, model, input_price_per_1m, output_price_per_1m,
     cache_creation_price_per_1m, cache_read_price_per_1m, notes)
VALUES
    ('mistral', 'mistral-small-latest', 0.10, 0.30, NULL, NULL, 'Mistral Small'),
    ('mistral', 'mistral-large-latest', 2.00, 6.00, NULL, NULL, 'Mistral Large'),
    ('mistral', 'mistral-embed', 0.10, 0.00, NULL, NULL, 'Mistral Embed'),
    ('ollama', 'llama3.2', 0.00, 0.00, NULL, NULL, 'Local Ollama llama3.2'),
    ('ollama', 'mistral', 0.00, 0.00, NULL, NULL, 'Local Ollama mistral'),
    ('ollama', 'nomic-embed-text', 0.00, 0.00, NULL, NULL, 'Local Ollama embeddings'),
    ('azure', 'gpt-4o', 2.50, 10.00, NULL, NULL, 'Azure OpenAI deployment placeholder (gpt-4o)'),
    ('azure', 'gpt-4o-mini', 0.15, 0.60, NULL, NULL, 'Azure OpenAI deployment placeholder (gpt-4o-mini)'),
    ('groq', 'openai/gpt-oss-20b', 0.075, 0.30, NULL, NULL, 'Groq GPT-OSS 20B'),
    ('groq', 'openai/gpt-oss-120b', 0.15, 0.60, NULL, NULL, 'Groq GPT-OSS 120B')
ON CONFLICT DO NOTHING;
