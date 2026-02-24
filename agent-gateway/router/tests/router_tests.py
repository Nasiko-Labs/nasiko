import json

import httpx
import requests

ROUTER_URL = "http://localhost:8081/router"
# TOKEN = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJ0b2tlbl9pZCI6IjJkNmM3MGIzLTdjMjMtNGNjNC1iZGRlLTMxMWU3MGZhZGU5OCIsImlzcyI6Im5hc2lrby1hdXRoIiwiaWF0IjoxNzY2Mzg0OTk1LCJleHAiOjE3NjY0MjgxOTV9.zA9DsqxhLN9OD1s_QnFsD6KLk8n0iqH-j65B1kPLRF4"
# HEADERS = {
#     "Authorization": f"Bearer {TOKEN}",
# }


def login_and_create_session():
    # Getting superuser credentials.
    with open("../orchestrator/superuser_credentials.json", "r") as f:
        superuser_credentials = json.load(f)

    try:
        payload = {
            "access_key": superuser_credentials["access_key"],
            "access_secret": superuser_credentials["access_secret"],
            "tenant_id": "default",
        }
    except Exception as e:
        print(f"Failed to load superuser credentials: {e}")
        return

    # Logging in and generating access token.
    login_url = "http://localhost:8082/auth/users/login"
    try:
        with httpx.Client(timeout=30) as client:
            response = client.post(login_url, json=payload)
            response.raise_for_status()

        data = response.json()
        access_token = data["token"]

    except httpx.RequestError as e:
        print(f"Failed to log in: {e}")
        return

    # Creating session.
    create_session_url = "http://localhost:8000/api/v1/chat/session"
    try:
        with httpx.Client(timeout=30) as client:
            response = client.post(
                create_session_url, headers={"Authorization": f"Bearer {access_token}"}
            )
            response.raise_for_status()

        data = response.json()

        session_id = data["data"]["session_id"]

    except httpx.RequestError as e:
        print(f"Failed to create a session: {e}")
        return

    return session_id, access_token


def test_router():
    queries = [
        "Generate image of Sauron wearing the One Ring.",
        "Does this email comply with company security policies?",
        "Break down the structure of this repo and explain what it does.",
        "How many rupees are in 1.43 dollars?",
    ]

    for query in enumerate(queries):
        data = {
            "session_id": "test-session",
            "query": query,
        }

        try:
            with requests.post(
                ROUTER_URL,
                headers=HEADERS,
                data=data,
                files=[],
                stream=True,
            ) as response:
                response.raise_for_status()
                print()
                print("---------------RESPONSE FROM AGENT---------------")
                for line in response.iter_lines():
                    if line:
                        obj = json.loads(line.decode("utf-8"))
                        print(obj)
            print()

        except requests.exceptions.RequestException as e:
            print(f"Request failed: {e}")


def test_router_multiturn(session_id, access_token):
    queries = [
        "How many rupees are in 1.43 dollars?",
        # "1 Euro = ? Rupees?",
        "Does the following email requesting a refund for my expenses for the US travel comply with company security policies?",
    ]

    # Assuming the container exposes a routing endpoint at http://localhost:8081/router
    data = {
        "session_id": session_id,
        "query": queries[0],
    }

    headers = {"Authorization": f"Bearer {access_token}"}
    route = None
    try:
        with requests.post(
            ROUTER_URL,
            headers=headers,
            data=data,
            files=[],
            stream=True,
        ) as response:
            response.raise_for_status()
            print()
            print("---------------RESPONSE FROM AGENT---------------")
            for line in response.iter_lines():
                if line:
                    obj = json.loads(line.decode("utf-8"))
                    print(obj)
                    if not obj["is_int_response"]:
                        route = obj["url"]
        print()

    except requests.exceptions.RequestException as e:
        print(f"First request failed: {e}")

    try:
        data = {
            "session_id": "6f5d22b9654b4d0b80697f1ad3e87686",
            "query": queries[1],
            "route": route,
        }
        with requests.post(
            ROUTER_URL,
            headers=headers,
            data=data,
            files=[],
            stream=True,
        ) as response:
            response.raise_for_status()
            print()
            print("---------------RESPONSE FROM AGENT---------------")
            for line in response.iter_lines():
                if line:
                    obj = json.loads(line.decode("utf-8"))
                    print(obj)
        print()

    except requests.exceptions.RequestException as e:
        print(f"Second request failed: {e}")


def test_router_with_files():
    queries = [
        "Summarise this research paper?",
        "Who are the authors of this file?",
    ]

    # Assuming the container exposes a routing endpoint at http://localhost:2000/router
    url = "http://localhost:2000/router"
    route = None

    payload = {
        "session_id": str(1),
        "query": queries[0],
        "route": route,
    }

    files = [
        (
            "files",
            ("test_file.pdf", open("tests/test_file.pdf", "rb"), "application/pdf"),
        )
    ]

    try:
        with requests.post(url, data=payload, files=files, stream=True) as response:
            response.raise_for_status()
            print()
            print("---------------RESPONSE FROM AGENT---------------")
            for line in response.iter_lines():
                if line:
                    obj = json.loads(line.decode("utf-8"))
                    print(obj)
                    if not obj["is_int_response"]:
                        route = obj["url"]
        print()

    except requests.exceptions.RequestException as e:
        print(f"First request failed: {e}")

    payload = {
        "session_id": str(1),
        "query": queries[1],
        "route": route,
    }

    try:
        with requests.post(url, data=payload, files=files, stream=True) as response:
            response.raise_for_status()
            print()
            print("---------------RESPONSE FROM AGENT---------------")
            for line in response.iter_lines():
                if line:
                    obj = json.loads(line.decode("utf-8"))
                    print(obj)
                    if not obj["is_int_response"]:
                        route = obj["url"]
        print()

    except requests.exceptions.RequestException as e:
        print(f"First request failed: {e}")

    finally:
        files[0][1][1].close()


def test_router_quality():
    """Comment out the code to send request to agents in main.py of src before running this test."""

    queries = [
        # compliance-checker
        {
            "query": "Does this vendor contract comply with our procurement policy?",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Review this employee handbook draft for compliance issues.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Check if this financial disclosure meets regulatory standards.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Identify compliance risks in this data privacy policy.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Ensure this sales pitch deck adheres to company branding rules.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Does this email about customer data sharing follow GDPR rules?",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Check compliance of this quarterly report with internal guidelines.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Review this vendor NDA for missing compliance clauses.",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Are there any red flags in this merger proposal regarding compliance?",
            "agent_name": "compliance-checker",
        },
        {
            "query": "Check if this cybersecurity report aligns with ISO standards.",
            "agent_name": "compliance-checker",
        },
        # document-expert
        {
            "query": "Summarize this 50-page annual report into 10 bullet points.",
            "agent_name": "document-expert",
        },
        {
            "query": "Provide a one-paragraph summary of this research paper.",
            "agent_name": "document-expert",
        },
        {
            "query": "Highlight the main findings in this audit report.",
            "agent_name": "document-expert",
        },
        {
            "query": "Condense this 2-hour meeting transcript into key action items.",
            "agent_name": "document-expert",
        },
        {
            "query": "Extract the executive summary from this long policy document.",
            "agent_name": "document-expert",
        },
        {
            "query": "Give me the top 3 takeaways from this industry analysis.",
            "agent_name": "document-expert",
        },
        {
            "query": "Turn this conference proceedings into a summary for executives.",
            "agent_name": "document-expert",
        },
        {
            "query": "Provide a TL;DR version of this article for internal newsletter.",
            "agent_name": "document-expert",
        },
        {
            "query": "What are the risks mentioned in this project proposal?",
            "agent_name": "document-expert",
        },
        {
            "query": "Summarize the client feedback document into 5 key themes.",
            "agent_name": "document-expert",
        },
        # github-agent
        {
            "query": "Summarize the functionality of https://github.com/pallets/flask.",
            "agent_name": "github-agent",
        },
        {
            "query": "List the main dependencies of https://github.com/psf/requests.",
            "agent_name": "github-agent",
        },
        {
            "query": "Does https://github.com/tensorflow/tensorflow have good documentation?",
            "agent_name": "github-agent",
        },
        {
            "query": "Break down the folder structure of https://github.com/fastai/fastai.",
            "agent_name": "github-agent",
        },
        {
            "query": "Fetch the latest 5 open PRs in https://github.com/tiangolo/fastapi.",
            "agent_name": "github-agent",
        },
        {
            "query": "Summarize the latest closed PRs in https://github.com/django/django.",
            "agent_name": "github-agent",
        },
        {
            "query": "Identify the main programming languages used in https://github.com/scikit-learn/scikit-learn.",
            "agent_name": "github-agent",
        },
        {
            "query": "Check if https://github.com/huggingface/transformers has contribution guidelines.",
            "agent_name": "github-agent",
        },
        {
            "query": "Explain the purpose of https://github.com/ethereum/go-ethereum.",
            "agent_name": "github-agent",
        },
        {
            "query": "Does https://github.com/pytorch/pytorch have tests for its modules?",
            "agent_name": "github-agent",
        },
        # translator
        {
            "query": "Translate this client proposal from English to German.",
            "agent_name": "translator",
        },
        {
            "query": "Convert this technical manual from Japanese into English.",
            "agent_name": "translator",
        },
        {
            "query": "Provide a Spanish translation of this compliance report.",
            "agent_name": "translator",
        },
        {
            "query": "Translate this Chinese contract into English.",
            "agent_name": "translator",
        },
        {
            "query": "Translate this French press release into English.",
            "agent_name": "translator",
        },
        {
            "query": "Convert this English product brochure into Italian.",
            "agent_name": "translator",
        },
        {
            "query": "Translate this Russian whitepaper into English.",
            "agent_name": "translator",
        },
        {
            "query": "Provide an Arabic translation of this HR policy.",
            "agent_name": "translator",
        },
        {
            "query": "Translate this German email into English.",
            "agent_name": "translator",
        },
        {
            "query": "Translate this Spanish newsletter into French.",
            "agent_name": "translator",
        },
    ]

    # queries = [
    #     # document-expert
    #     {
    #         "query": "Summarize this 50-page annual report into 10 bullet points.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Provide a one-paragraph summary of this research paper.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Highlight the main findings in this audit report.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Condense this 2-hour meeting transcript file into key action items.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Extract the executive summary from this long policy document.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Give me the top 3 takeaways from this industry analysis.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Turn this conference proceedings into a summary for executives.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Provide a TL;DR version of this article for internal newsletter.",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "What are the risks mentioned in this project proposal?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Summarize the client feedback document into 5 key themes.",
    #         "agent_name": "document-expert",
    #     },
    # ]

    # queries = [
    #     {
    #         "query": "What did Bhargav say about the project proposal in the this file?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Does this research paper prove that symmetric tensor rank is NP-hard?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "According to this email, when is the standup meeting?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "What is the purpose of this document?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "What is the main topic of this document?",
    #         "agent_name": "document-expert",
    #     },
    #     {
    #         "query": "Who are the authors of this paper?",
    #         "agent_name": "document-expert",
    #     },
    # ]
    for query in queries:
        payload = {
            "session_id": "1",
            "query": query["query"],
            "route": None,
        }

        try:
            response = requests.post(
                "http://localhost:2000/router",
                data=payload,
                files=[],
                stream=True,
            )
            response.raise_for_status()

            print()
            print("---------------RESPONSE FROM AGENT---------------")
            for line in response.iter_lines():
                if line:
                    obj = json.loads(line.decode("utf-8"))
                    print(obj["message"])
            print("Expected Agent Name:", query["agent_name"])

        except requests.exceptions.RequestException as e:
            print(f"Request failed: {e}")


if __name__ == "__main__":
    session_id, access_token = login_and_create_session()
    # test_router()
    test_router_multiturn(session_id, access_token)
    # test_router_with_files()
