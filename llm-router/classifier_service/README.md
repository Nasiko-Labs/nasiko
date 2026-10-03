# Local request classifier

1. Run `notebooks/request_classifier_knn.ipynb` from the repository root in a Python 3.10+ environment. Annotate public LLMRouter benchmark rows with Nasiko request type, complexity, source, and paraphrase group before providing them with `LLMROUTER_LABELLED_DATA`. Do not include private evaluation cases or user prompts.
2. The notebook exports `llm-router/artifacts/request_classifier.joblib`. The artifact stores KNN vectors and labels, calibration, and metadata; it does not store the example query text.
3. Install the pinned runtime dependencies with `python -m pip install -r llm-router/classifier_service/requirements.txt` and run tests from this directory with `pytest test_app.py`.
4. Start the local stack with `docker compose -f docker-compose.yml -f llm-router/docker-compose.classifier.yml up --build`. The overlay opts the server into `llmrouter_knn`; the service is reachable only on the Compose network and publishes no host port.

The service loads the Longformer revision named in the artifact once at startup. The router's default remains `REQUEST_CLASSIFIER_BACKEND=regex` outside the overlay.
