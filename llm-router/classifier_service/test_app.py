import numpy as np

from app import classify_with_bundle


class FakeKnn:
    def __init__(self, indices, distances, weights="uniform"):
        self.indices = np.asarray([indices])
        self.distances = np.asarray([distances], dtype=float)
        self.n_neighbors = len(indices)
        self.weights = weights

    def kneighbors(self, _vector, n_neighbors):
        assert n_neighbors == self.n_neighbors
        return self.distances, self.indices


class IdentityCalibrator:
    def predict(self, values):
        return np.asarray(values)


def test_tied_neighbor_votes_use_stable_request_type_order():
    bundle = {
        "request_type_knn": FakeKnn([0, 1], [0.4, 0.4]),
        "request_type_labels": ["general", "code_generation"],
        "confidence_calibrator": IdentityCalibrator(),
        "complexity_knn": FakeKnn([0, 1], [0.3, 0.3]),
        "complexity_labels": [2, 4],
    }
    result = classify_with_bundle("same query", bundle, embed=lambda _text: np.zeros((1, 3)))
    assert result == {"request_type": "code_generation", "complexity": 3, "confidence": 0.5}


def test_distance_weighting_is_deterministic_and_complexity_is_bounded():
    bundle = {
        "request_type_knn": FakeKnn([0, 1, 2], [0.1, 0.8, 0.9], weights="distance"),
        "request_type_labels": ["writing", "general", "general"],
        "confidence_calibrator": IdentityCalibrator(),
        "complexity_knn": FakeKnn([0, 1], [0.1, 0.9], weights="distance"),
        "complexity_labels": [9, 5],
    }
    classify = lambda: classify_with_bundle(
        "stable query", bundle, embed=lambda _text: np.zeros((1, 3))
    )
    first = classify()
    assert first == classify()
    assert first["request_type"] == "writing"
    assert first["complexity"] == 5
    assert 0 <= first["confidence"] <= 1
