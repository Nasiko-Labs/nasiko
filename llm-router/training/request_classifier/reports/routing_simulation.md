# Routing simulation (219 cases × 200 seeds, cold start; cost proxy, not answer quality)

| system | T1 / T2 / T3 share | gold cx≥4 → T3 | gold cx≤2 → T1 | relative cost | deterministic |
|---|---|---|---|---|---|
| regex (today) | 7.4% / 41.9% / 50.7% | 50.6% | 6.3% | 1.000 | yes |
| model, complexity routing off | 13.8% / 50.7% / 35.5% | 28.9% | 10.7% | 1.399 | yes |
| model, complexity routing on | 11.2% / 44.3% / 44.4% | 7.4% | 2.7% | 1.218 | yes |
