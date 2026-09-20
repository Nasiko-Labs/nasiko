"""Tests for the EvidenceBridge Agent."""
import pytest
import asyncio
from unittest.mock import patch, MagicMock

from main import extract_supplier_name, format_response, EvidenceBridgeExecutor
from a2a.server.agent_execution import RequestContext
from a2a.server.events import EventQueue
from a2a.types import TaskState, Message, Part

def test_extract_supplier_name():
    assert extract_supplier_name("Verify supplier Acme Technologies") == "Acme Technologies"
    assert extract_supplier_name("verify supplier Globex Corp.") == "Globex Corp"
    assert extract_supplier_name("Verify Initech") == "Initech"
    assert extract_supplier_name("Stark Industries") == "Stark Industries"
    assert extract_supplier_name("v") is None

def test_format_response_success():
    data = {
        "supplier_name": "Acme",
        "status": "CONSISTENT",
        "evidence_urls": ["http://example.com"],
        "conflicts": [],
        "recommended_actions": ["Proceed with payment"],
        "identity_ambiguity": False
    }
    resp = format_response(data)
    assert "Supplier Verification Report: Acme" in resp
    assert "**Status:** CONSISTENT" in resp
    assert "http://example.com" in resp
    assert "Proceed with payment" in resp
    assert "Identity Ambiguity" not in resp
    assert "Conflicts" not in resp

def test_format_response_ambiguity_and_conflicts():
    data = {
        "supplier_name": "Acme",
        "status": "MISMATCH",
        "evidence_urls": ["http://example.com/1", "http://example.com/2"],
        "conflicts": ["Address mismatch"],
        "recommended_actions": ["Manual review required"],
        "identity_ambiguity": True
    }
    resp = format_response(data)
    assert "**Status:** MISMATCH" in resp
    assert "Identity Ambiguity:" in resp
    assert "Address mismatch" in resp
    assert "Manual review required" in resp

@pytest.mark.asyncio
async def test_backend_call_success(monkeypatch):
    monkeypatch.setenv("EVIDENCEBRIDGE_API_URL", "http://test-api")
    
    executor = EvidenceBridgeExecutor()
    
    context = MagicMock(spec=RequestContext)
    context.get_user_input.return_value = "Verify supplier TestCorp"
    context.message = Message(
        message_id="123",
        role="ROLE_USER",
        parts=[Part(text="Verify supplier TestCorp")]
    )
    context.current_task = None
    
    event_queue = MagicMock(spec=EventQueue)
    event_queue.enqueue_event = MagicMock(return_value=asyncio.Future())
    event_queue.enqueue_event.return_value.set_result(None)
    
    mock_response = MagicMock()
    mock_response.json.return_value = {
        "supplier_name": "TestCorp",
        "status": "CONSISTENT"
    }
    mock_response.raise_for_status.return_value = None
    
    with patch("httpx.AsyncClient.post") as mock_post:
        mock_post.return_value = mock_response
        await executor.execute(context, event_queue)
        
        mock_post.assert_called_once_with("http://test-api", json={"supplier_name": "TestCorp"})
        
    # Check that events were enqueued
    assert event_queue.enqueue_event.call_count > 0
    
    # Check final status
    final_call_args = event_queue.enqueue_event.call_args_list[-1][0][0]
    assert final_call_args.status.state == TaskState.TASK_STATE_COMPLETED

@pytest.mark.asyncio
async def test_backend_call_failure(monkeypatch):
    monkeypatch.setenv("EVIDENCEBRIDGE_API_URL", "http://test-api")
    
    executor = EvidenceBridgeExecutor()
    
    context = MagicMock(spec=RequestContext)
    context.get_user_input.return_value = "Verify supplier TestCorp"
    context.message = Message(
        message_id="123",
        role="ROLE_USER",
        parts=[Part(text="Verify supplier TestCorp")]
    )
    context.current_task = None
    
    event_queue = MagicMock(spec=EventQueue)
    event_queue.enqueue_event = MagicMock(return_value=asyncio.Future())
    event_queue.enqueue_event.return_value.set_result(None)
    
    with patch("httpx.AsyncClient.post", side_effect=Exception("API Error")) as mock_post:
        await executor.execute(context, event_queue)
        
    final_call_args = event_queue.enqueue_event.call_args_list[-1][0][0]
    assert final_call_args.status.state == TaskState.TASK_STATE_FAILED
    assert "Failed to contact EvidenceBridge backend" in final_call_args.status.message.parts[0].text
