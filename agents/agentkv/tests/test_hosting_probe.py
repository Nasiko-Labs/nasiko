from io import BytesIO
import unittest
from unittest.mock import patch
from infra.nasiko_probe_guest import request


class ProbeTests(unittest.TestCase):
    def test_nasiko_health_is_plain_text(self):
        with patch("urllib.request.urlopen", return_value=BytesIO(b"ok")):
            self.assertEqual(request("/health"), {"healthy": True})

    def test_login_still_requires_json(self):
        with patch("urllib.request.urlopen", return_value=BytesIO(b'{"token":"fixture"}')):
            self.assertEqual(request("/api/auth/login", {"username": "fixture"})["token"], "fixture")
