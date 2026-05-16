"""Unit tests for platform logs service validation."""

import pytest

from app.service.platform_logs_service import PlatformLogsService


class _FakeRepo:
    async def list_logs(self, level=None, limit=100, skip=0):
        return []

    async def count_logs(self, level=None):
        return 0


@pytest.mark.asyncio
async def test_list_logs_rejects_invalid_level():
    service = PlatformLogsService(_FakeRepo(), None)
    with pytest.raises(ValueError, match="Invalid log level"):
        await service.list_logs(level="DEBUG")
