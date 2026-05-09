from unittest.mock import patch

from router.src.core.routing_engine import RoutingEngine


def test_single_agent_route_does_not_require_embedding_or_llm_calls():
    with patch.object(RoutingEngine, "_create_llm", return_value=object()):
        engine = RoutingEngine()

    with patch.object(
        RoutingEngine,
        "_create_embedding_model",
        side_effect=AssertionError("embeddings should be lazy"),
    ):
        first, scores, second, output = engine.route_query(
            message="Translate hello to French",
            conversation_history=[],
            agent_cards=[
                {
                    "name": "Translator Agent",
                    "description": "Translate text between languages",
                }
            ],
            vectorstore=None,
        )

    assert first == ["Translator Agent"]
    assert second == ["Translator Agent"]
    assert scores == [1.0]
    assert output.agent_name == "Translator Agent"
