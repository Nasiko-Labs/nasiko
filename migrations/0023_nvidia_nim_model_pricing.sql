-- NVIDIA NIM catalog rows for LLM Router UI (Issue 4 follow-on).

INSERT INTO model_pricing
    (provider, model, input_price_per_1m, output_price_per_1m,
     cache_creation_price_per_1m, cache_read_price_per_1m, notes)
VALUES
    ('nvidia', 'mistralai/mistral-nemotron', 0.00, 0.00, NULL, NULL, 'Mistral Nemotron via NVIDIA NIM'),
    ('nvidia', 'nvidia/mistral-nemo-minitron-8b-8k-instruct', 0.00, 0.00, NULL, NULL, 'NVIDIA Mistral Nemo Minitron 8B'),
    ('nvidia', 'nvidia/llama-3.1-nemotron-70b-instruct', 0.00, 0.00, NULL, NULL, 'NVIDIA Nemotron 70B Instruct')
ON CONFLICT DO NOTHING;
