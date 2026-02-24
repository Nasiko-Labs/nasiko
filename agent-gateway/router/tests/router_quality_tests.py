# pytest routing_big_tests.py -v -s (-s to see the output on terminal)
import json
import os
import random
from pathlib import Path
from typing import List, Dict, Any

import numpy as np
from tqdm import tqdm
from langchain_core.embeddings import Embeddings
from langchain_openai import OpenAIEmbeddings
from langchain_community.vectorstores import FAISS


from router.src.core.routing_engine import RoutingEngine
from router.src.config import settings


AGENT_CARDS_DIR = "router/data/agent_cards"
QUERIES_RESPONSES_DIR = "router/data/query_response_pairs"
REGISTRIES_FILE = "router/data/registries.json"
TEST_CASES_FILE = "router/data/test_cases.json"
EMBEDDINGS_FILE = "router/data/agent_card_embeddings.npy"
SAMPLED_REGISTRIES_FILE = "router/data/sampled_registries.json"
PROCESSED_CASES_FILE = "router/data/processed_cases.json"
RESULTS_DIR = "router/results"
FAILURES_DIR = "router/results/failures"
RESULTS_FILE = "router/results/results.txt"

# Fixed seed for reproducibility
RANDOM_SEED = 42

# Size ranges for registry selection
# SIZE_RANGES = [
#     (">=1000", lambda s: s >= 1000),
#     ("500-999", lambda s: 500 <= s <= 999),
#     ("250-499", lambda s: 250 <= s <= 499),
#     ("211-249", lambda s: 211 <= s <= 249),
#     ("176-210", lambda s: 176 <= s <= 210),
#     ("141-175", lambda s: 141 <= s <= 175),
#     ("106-140", lambda s: 106 <= s <= 140),
#     ("71-105", lambda s: 71 <= s <= 105),
#     ("36-70", lambda s: 36 <= s <= 70),
#     ("2-35", lambda s: 2 <= s <= 35),
# ]

SIZE_RANGES = [
    # ("71-105", lambda s: 71 <= s <= 105),
    ("36-70", lambda s: 36 <= s <= 70),
    ("2-35", lambda s: 2 <= s <= 35),
]


def load_agent_cards():
    agent_cards_path = Path(AGENT_CARDS_DIR)
    if not agent_cards_path.exists():
        raise FileNotFoundError(
            f"Agent cards directory '{AGENT_CARDS_DIR}' does not exist."
        )
    json_files = sorted(
        agent_cards_path.glob("agent_card_*.json"),
        key=lambda f: int(f.stem.split("_")[-1]),
    )
    agent_cards = []
    for json_file in json_files:
        try:
            with open(json_file, "r", encoding="utf-8") as f:
                agent_card = json.load(f)
                agent_cards.append(agent_card)
        except json.JSONDecodeError as e:
            print(f"Error parsing JSON in {json_file.name}: {e}")
        except Exception as e:
            print(f"Error reading {json_file.name}: {e}")
    print(f"Loaded {len(agent_cards)} agent cards from '{AGENT_CARDS_DIR}'.")
    return agent_cards


def load_queries_and_responses():
    """
    Loads queries and responses from JSON files in the directory specified by
    QUERIES_RESPONSES_DIR.

    Returns a list of dictionaries, where each dictionary contains a query and
    the corresponding responses.

    Raises a FileNotFoundError if the directory specified by QUERIES_RESPONSES_DIR
    does not exist.
    """
    queries_and_responses_path = Path(QUERIES_RESPONSES_DIR)
    if not queries_and_responses_path.exists():
        raise FileNotFoundError(
            f"Queries directory '{QUERIES_RESPONSES_DIR}' does not exist."
        )
    json_files = sorted(queries_and_responses_path.glob("queries_*.json"))
    queries = []
    for json_file in json_files:
        try:
            with open(json_file, "r", encoding="utf-8") as f:
                query = json.load(f)
                queries.append(query)
        except json.JSONDecodeError as e:
            print(f"Error parsing JSON in {json_file.name}: {e}")
        except Exception as e:
            print(f"Error reading {json_file.name}: {e}")
    print(f"Loaded {len(queries)} queries from '{QUERIES_RESPONSES_DIR}'.")
    return queries


def prepare_agent_card(agent_card: Dict[str, Any]) -> str:
    text = (
        f"Agent name: {agent_card['name']}\nDescription: {agent_card['description']}\n"
    )

    for i, skill in enumerate(agent_card["skills"]):
        text += f"Skill {i}: {skill['name']}\nDescription: {skill["description"]}"

    return text


def build_vecstore_from_vecs(
    agent_cards: List[Dict[str, Any]],
    vectors: List[List[float]],
    embeddings: Embeddings,  # must be an Embeddings object (e.g., OpenAIEmbeddings())
) -> FAISS:
    if len(agent_cards) != len(vectors):
        raise ValueError("Number of documents and vectors must be the same.")

    texts = list(map(prepare_agent_card, agent_cards))
    text_embedding_pairs = [(text, vec) for text, vec in zip(texts, vectors)]
    metadatas = [{"name": card["name"]} for card in agent_cards]

    vector_store = FAISS.from_embeddings(
        text_embedding_pairs, embeddings, metadatas=metadatas
    )

    return vector_store


def compute_agent_card_embeddings(agent_cards, embeddings):
    documents = []

    for card in agent_cards:
        documents.append(prepare_agent_card(card))

    return embeddings.embed_documents(documents)


def load_registries():
    with open(REGISTRIES_FILE, "r", encoding="utf-8") as f:
        registries = json.load(f)
    return registries


def load_test_cases():
    with open(TEST_CASES_FILE, "r", encoding="utf-8") as f:
        test_cases = json.load(f)
    return test_cases


def load_agent_card_by_filename(filename: str) -> Dict[str, Any]:
    """Load a single agent card by its filename."""
    agent_card_path = Path(AGENT_CARDS_DIR) / filename
    with open(agent_card_path, "r", encoding="utf-8") as f:
        return json.load(f)


def load_query_response(filename: str, query_index: int) -> Dict[str, Any]:
    """Load query and response for a specific agent and query index."""
    query_response_path = os.path.join(QUERIES_RESPONSES_DIR, filename)
    with open(query_response_path, "r", encoding="utf-8") as f:
        data = json.load(f)
    # The file has a list of 5 query_response_pairs, return the one at query_index
    query_response = data["query_response_pairs"][query_index]
    return {
        "agent_name": data["agent_name"],
        "query": query_response["query"],
        "response": query_response["response"],
    }


def select_registries_by_size_ranges(
    registries: List[Dict[str, Any]], sample_fraction: float = 0.1
) -> List[int]:
    """
    Select 10% of registries from each size range using a fixed seed.
    Returns list of registry indices.
    """
    random.seed(RANDOM_SEED)

    selected_indices = []

    for range_name, range_filter in SIZE_RANGES:
        # Find all registries in this size range
        indices_in_range = [
            i for i, reg in enumerate(registries) if range_filter(reg["size"])
        ]

        # Select 10% of them (at least 1 if any exist)
        num_to_select = max(1, int(len(indices_in_range) * sample_fraction))
        selected = random.sample(
            indices_in_range, min(num_to_select, len(indices_in_range))
        )
        selected_indices.extend(selected)

        print(
            f"Size range {range_name}: selected {len(selected)}/{len(indices_in_range)} registries"
        )

    return selected_indices


def get_test_cases_for_registry(
    test_cases: List[Dict], registry_idx: int
) -> List[Dict]:
    """Get all test cases for a specific registry index."""
    return [tc for tc in test_cases if tc["registry_id"] == registry_idx]


def load_turn_data(
    query_data: Dict[str, Any], registry: Dict[str, Any]
) -> Dict[str, Any]:
    """
    Load all information needed to run a test case.
    Returns dict with agent_card, query, and response.

    Args:
        query_data: A query entry from test_cases.json containing agent_idx and query_index
        registry: The registry dict containing the agents list
    """
    agent_idx = query_data["agent_idx"]
    query_index = query_data["query_index"]

    # Get the agent card filename from the registry's agents list at agent_idx
    agent_entry = registry["agents"][agent_idx]
    agent_card_filename = agent_entry["agent_card"]
    query_filename = query_data["queries_file"]
    agent_card = load_agent_card_by_filename(agent_card_filename)

    # Load query and response from query_response file
    query_response_data = load_query_response(query_filename, query_index)

    # Assert that agent names match between agent card and query_response file
    assert agent_card["name"] == query_response_data["agent_name"], (
        f"Agent name mismatch: agent_card has '{agent_card['name']}' but "
        f"query_response file has '{query_response_data['agent_name']}'"
    )

    return {
        "agent_name": agent_card["name"],
        "Human Message": query_response_data["query"],
        "AI Message": query_response_data["response"],
        # "agent_idx": agent_idx,
        # "query_index": query_index,
    }


def load_testcase_data(
    testcase: Dict[str, Any], registry: Dict[str, Any]
) -> Dict[str, Any]:
    conversation = []
    for query in testcase["queries"]:
        query_data = load_turn_data(query, registry)
        conversation.append(query_data)

    return {
        "num_turns": testcase["num_queries"],
        "conversation": conversation,
    }


def get_agent_card_index_from_filename(filename: str) -> int:
    """Extract agent card index from filename like 'agent_card_123.json' -> 123."""
    base = filename.replace("agent_card_", "").replace(".json", "")
    return int(base)


def load_agent_cards_for_registry(registry: Dict[str, Any]) -> List[Dict[str, Any]]:
    """
    Load agent cards for agents in a specific registry.

    Args:
        registry: Registry dict containing agents list with agent_card filenames

    Returns:
        List of agent cards for agents in the registry
    """
    registry_agent_cards = []
    for agent_entry in registry["agents"]:
        agent_card_filename = agent_entry["agent_card"]
        agent_card = load_agent_card_by_filename(agent_card_filename)
        registry_agent_cards.append(agent_card)
    return registry_agent_cards


def build_vectorstore_for_registry(
    registry: Dict[str, Any],
    all_agent_cards: List[Dict[str, Any]],
    all_embeddings: np.ndarray,
    embeddings_model: Embeddings,
) -> FAISS:
    """
    Build a FAISS vectorstore for agents in a specific registry.

    Args:
        registry: Registry dict containing agents list with agent_card filenames
        all_agent_cards: List of all agent cards (indexed by their card number)
        all_embeddings: Numpy array of all agent card embeddings
        embeddings_model: Embeddings model instance for FAISS

    Returns:
        FAISS vectorstore for the registry's agents
    """
    registry_agent_cards = []
    registry_embeddings = []

    for agent_entry in registry["agents"]:
        agent_card_filename = agent_entry["agent_card"]
        agent_card_idx = get_agent_card_index_from_filename(agent_card_filename)
        registry_agent_cards.append(all_agent_cards[agent_card_idx])
        registry_embeddings.append(all_embeddings[agent_card_idx])

    return build_vecstore_from_vecs(
        registry_agent_cards, registry_embeddings, embeddings_model
    )


def get_size_range_for_registry(registry_size: int) -> str:
    """Get the size range label for a registry size."""
    for range_name, range_filter in SIZE_RANGES:
        if range_filter(registry_size):
            return range_name
    return "unknown"


def load_processed_cases() -> Dict[str, Any]:
    """Load processed cases from file if it exists."""
    if Path(PROCESSED_CASES_FILE).exists():
        with open(PROCESSED_CASES_FILE, "r", encoding="utf-8") as f:
            return json.load(f)
    return {"last_completed_registry_idx": -1, "completed_registry_indices": []}


def save_processed_cases(processed_cases: Dict[str, Any]):
    """Save processed cases to file."""
    with open(PROCESSED_CASES_FILE, "w", encoding="utf-8") as f:
        json.dump(processed_cases, f, indent=2)


def test_router_quality(embedding_file=EMBEDDINGS_FILE, resume=True):
    # Load registries.
    registries = load_registries()

    # Load test cases.
    test_cases = load_test_cases()

    # Select 10% of registries for each size range.
    selected_registry_indices = select_registries_by_size_ranges(
        registries, sample_fraction=0.1
    )

    # Save selected registries to file for reproducibility
    with open(SAMPLED_REGISTRIES_FILE, "w", encoding="utf-8") as f:
        json.dump(selected_registry_indices, f, indent=2)
    print(
        f"Saved {len(selected_registry_indices)} sampled registry indices to {SAMPLED_REGISTRIES_FILE}"
    )

    agent_cards = load_agent_cards()

    # If embeddings file does not exist, compute embeddings for all cards.
    if not Path(EMBEDDINGS_FILE).exists():
        agent_cards_embeddings = compute_agent_card_embeddings(
            agent_cards, OpenAIEmbeddings()
        )
        np.save(EMBEDDINGS_FILE, agent_cards_embeddings)

    # Load agent card embeddings.
    print("Loading agent card embeddings from agent_card_embeddings.npy")
    agent_cards_embeddings = np.load(EMBEDDINGS_FILE)
    print(
        f"Loaded {len(agent_cards_embeddings)} embeddings from agent_card_embeddings.npy"
    )

    # Create embeddings model for vectorstore
    embeddings_model = OpenAIEmbeddings()

    # Create output directories
    Path(RESULTS_DIR).mkdir(parents=True, exist_ok=True)
    Path(FAILURES_DIR).mkdir(parents=True, exist_ok=True)

    # Load processed cases for resume functionality
    processed_cases = (
        load_processed_cases()
        if resume
        else {"last_completed_registry_idx": -1, "completed_registry_indices": []}
    )
    completed_registry_indices = set(
        processed_cases.get("completed_registry_indices", [])
    )

    # Filter out already processed registries if resuming
    registries_to_process = [
        idx
        for idx in selected_registry_indices
        if idx not in completed_registry_indices
    ]

    if resume and len(registries_to_process) < len(selected_registry_indices):
        print(
            f"Resuming: skipping {len(selected_registry_indices) - len(registries_to_process)} already processed registries"
        )

    # Initialize tracking variables
    total_turns = 0
    failed_turns = 0
    total_convs = 0
    failed_convs = 0

    # Track by size range
    stats_by_size_range = {
        range_name: {
            "total_turns": 0,
            "failed_turns": 0,
            "total_convs": 0,
            "failed_convs": 0,
        }
        for range_name, _ in SIZE_RANGES
    }

    # Track failures by turn index
    turn_idx_stats = {}  # turn_idx -> {"total": count, "failed": count}

    # Initialize routing engine
    router = RoutingEngine()

    # Process each registry
    for registry_idx in tqdm(registries_to_process, desc="Processing registries"):
        registry = registries[registry_idx]
        registry_size = registry["size"]
        size_range = get_size_range_for_registry(registry_size)

        # Build vectorstore for this registry
        vectorstore = build_vectorstore_for_registry(
            registry, agent_cards, agent_cards_embeddings, embeddings_model
        )

        # Load agent cards for this registry
        registry_agent_cards = load_agent_cards_for_registry(registry)

        # Get test cases for this registry
        registry_test_cases = get_test_cases_for_registry(test_cases, registry_idx)

        # Process each test case (conversation)
        for test_case in registry_test_cases:
            conversation_history = []
            conv_failed = False
            failed_turns_in_conv = []

            total_convs += 1
            stats_by_size_range[size_range]["total_convs"] += 1

            for turn_idx, query in enumerate(test_case["queries"]):
                turn_data = load_turn_data(query, registry)

                # Track turn index stats
                if turn_idx not in turn_idx_stats:
                    turn_idx_stats[turn_idx] = {"total": 0, "failed": 0}
                turn_idx_stats[turn_idx]["total"] += 1

                total_turns += 1
                stats_by_size_range[size_range]["total_turns"] += 1

                # Call the router
                try:
                    (
                        first_shortlist,
                        similarity_score,
                        second_shortlist,
                        router_output,
                    ) = router.route_query(
                        message=turn_data["Human Message"],
                        conversation_history=conversation_history,
                        agent_cards=registry_agent_cards,
                        vectorstore=vectorstore,
                    )

                    selected_agent = router_output.agent_name
                    correct_agent = turn_data["agent_name"]

                    # Check if router selected correct agent
                    if selected_agent != correct_agent:
                        failed_turns += 1
                        stats_by_size_range[size_range]["failed_turns"] += 1
                        turn_idx_stats[turn_idx]["failed"] += 1
                        conv_failed = True

                        failed_turns_in_conv.append(
                            {
                                "turn_idx": turn_idx,
                                "correct_agent": correct_agent,
                                "router_selected_agent": selected_agent,
                                "first_shortlist": first_shortlist,
                                "similarity_score": similarity_score,
                                "second_shortlist": second_shortlist,
                                "human_message": turn_data["Human Message"],
                                "conversation_history": conversation_history.copy(),
                            }
                        )

                except Exception as e:
                    print(
                        f"Error routing turn {turn_idx} in registry {registry_idx}: {e}"
                    )
                    failed_turns += 1
                    stats_by_size_range[size_range]["failed_turns"] += 1
                    turn_idx_stats[turn_idx]["failed"] += 1
                    conv_failed = True

                    failed_turns_in_conv.append(
                        {
                            "turn_idx": turn_idx,
                            "correct_agent": turn_data["agent_name"],
                            "router_selected_agent": None,
                            "error": str(e),
                            "human_message": turn_data["Human Message"],
                            "conversation_history": conversation_history.copy(),
                        }
                    )

                # Append turn to conversation history
                conversation_history.append(
                    {"role": "Human", "content": turn_data["Human Message"]}
                )
                conversation_history.append(
                    {"role": "Assistant", "content": turn_data["AI Message"]}
                )

            # If conversation had any failures, log to failure file
            if conv_failed:
                failed_convs += 1
                stats_by_size_range[size_range]["failed_convs"] += 1

                # Rebuild all turns data for logging
                all_turns_data = []
                for i, query in enumerate(test_case["queries"]):
                    td = load_turn_data(query, registry)
                    all_turns_data.append(
                        {
                            "turn_idx": i,
                            "human_message": td["Human Message"],
                            "ai_message": td["AI Message"],
                            "agent_name": td["agent_name"],
                        }
                    )

                failure_data = {
                    "registry_idx": registry_idx,
                    "registry_size": registry_size,
                    "size_range": size_range,
                    "all_turns": all_turns_data,
                    "failed_turns": failed_turns_in_conv,
                }

                # Save failure to file
                failure_filename = f"failure_reg{registry_idx}_conv{total_convs}.json"
                failure_path = Path(FAILURES_DIR) / failure_filename
                with open(failure_path, "w", encoding="utf-8") as f:
                    json.dump(failure_data, f, indent=2)

        # Update processed cases after each registry is complete
        completed_registry_indices.add(registry_idx)
        processed_cases["completed_registry_indices"] = list(completed_registry_indices)
        processed_cases["last_completed_registry_idx"] = registry_idx
        save_processed_cases(processed_cases)

    # Write results file
    with open(RESULTS_FILE, "w", encoding="utf-8") as f:
        f.write("=" * 60 + "\n")
        f.write("ROUTER QUALITY TEST RESULTS\n")
        f.write("=" * 60 + "\n\n")

        # Overall stats
        f.write("OVERALL STATISTICS\n")
        f.write("-" * 40 + "\n")
        f.write(f"Turn accuracy: {total_turns - failed_turns}/{total_turns}")
        if total_turns > 0:
            f.write(f" ({(total_turns - failed_turns) / total_turns * 100:.2f}%)\n")
        else:
            f.write("\n")
        f.write(f"Conversation accuracy: {total_convs - failed_convs}/{total_convs}")
        if total_convs > 0:
            f.write(f" ({(total_convs - failed_convs) / total_convs * 100:.2f}%)\n")
        else:
            f.write("\n")
        f.write("\n")

        # Stats by size range
        f.write("STATISTICS BY SIZE RANGE\n")
        f.write("-" * 40 + "\n")
        for range_name, _ in SIZE_RANGES:
            stats = stats_by_size_range[range_name]
            if stats["total_turns"] > 0:
                turn_acc = (
                    (stats["total_turns"] - stats["failed_turns"])
                    / stats["total_turns"]
                    * 100
                )
                conv_acc = (
                    (stats["total_convs"] - stats["failed_convs"])
                    / stats["total_convs"]
                    * 100
                    if stats["total_convs"] > 0
                    else 0
                )
                f.write(f"\n{range_name}:\n")
                f.write(
                    f"  Turn accuracy: {stats['total_turns'] - stats['failed_turns']}/{stats['total_turns']} ({turn_acc:.2f}%)\n"
                )
                f.write(
                    f"  Conv accuracy: {stats['total_convs'] - stats['failed_convs']}/{stats['total_convs']} ({conv_acc:.2f}%)\n"
                )
        f.write("\n")

        # Stats by turn index
        f.write("STATISTICS BY TURN INDEX\n")
        f.write("-" * 40 + "\n")
        for turn_idx in sorted(turn_idx_stats.keys()):
            stats = turn_idx_stats[turn_idx]
            if stats["total"] > 0:
                acc = (stats["total"] - stats["failed"]) / stats["total"] * 100
                f.write(
                    f"Turn {turn_idx}: {stats['total'] - stats['failed']}/{stats['total']} ({acc:.2f}%)\n"
                )

    print(f"\nResults written to {RESULTS_FILE}")
    print(f"Failures written to {FAILURES_DIR}/")

    # Print summary to console
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"Turn accuracy: {total_turns - failed_turns}/{total_turns}", end="")
    if total_turns > 0:
        print(f" ({(total_turns - failed_turns) / total_turns * 100:.2f}%)")
    else:
        print()
    print(f"Conversation accuracy: {total_convs - failed_convs}/{total_convs}", end="")
    if total_convs > 0:
        print(f" ({(total_convs - failed_convs) / total_convs * 100:.2f}%)")
    else:
        print()

    return {
        "total_turns": total_turns,
        "failed_turns": failed_turns,
        "total_convs": total_convs,
        "failed_convs": failed_convs,
        "stats_by_size_range": stats_by_size_range,
        "turn_idx_stats": turn_idx_stats,
    }


# def test_routing(total_queries=5):
#     # Load agent queries
#     agent_cards = load_agent_cards()
#     embeddings = OpenAIEmbeddings()
#     agent_names = [agent["name"] for agent in agent_cards]
#     agent_cards_embeddings = embeddings.embed_documents(
#         [agent_card["description"] for agent_card in agent_cards]
#     )
#     # Create dict with keys as agent_ids and values as embeddings
#     name_embedding_dict = dict(zip(agent_names, agent_cards_embeddings))
#     num_agents = len(agent_cards)

#     # Load queries
#     queries = load_queries_and_responses()
#     assert len(agent_cards) == len(queries)

#     results = []
#     correct_count = 0
#     not_shortlisted_count = 0
#     total_time = 0

#     # Wrap your loop with tqdm
#     for i in tqdm(range(total_queries), desc="Routing requests"):
#         # pick one correct agent
#         correct_agent_idx = random.randint(0, num_agents - 1)
#         correct_agent = agent_cards[correct_agent_idx]
#         correct_agent_name = correct_agent["name"]

#         query = random.choice(queries[correct_agent_idx]["queries"])

#         # pick 9 random distractor agents
#         distractors = random.sample(
#             [a for a in agent_cards if a["name"] != correct_agent_name],
#             19,
#         )

#         # form 20 agent cards
#         test_agent_cards = [correct_agent] + [d for d in distractors]
#         truncated_agent_cards = truncate_agent_cards(test_agent_cards)
#         random.shuffle(truncated_agent_cards)

#         docs = []
#         vecs = []
#         for agent_card in truncated_agent_cards:
#             agent_name = agent_card["name"]
#             docs.append(
#                 Document(
#                     page_content=agent_card["description"],
#                     metadata={"name": agent_name},
#                 )
#             )
#             vecs.append(name_embedding_dict[agent_name])

#         vectorstore = build_faiss_from_precomputed(docs, vecs, embeddings)

#         start = time.time()
#         shortlisted_agents, router_output = router(
#             query, truncated_agent_cards, vectorstore
#         )
#         elapsed = time.time() - start
#         total_time += elapsed

#         if isinstance(router_output, RouterOutput):
#             agent_name = router_output.agent_name
#         elif isinstance(router_output, dict):
#             agent_name = router_output.get("agent_name")
#         else:
#             print(f"Error parsing router output: {e}")

#         if agent_name == correct_agent_name:
#             correct_count += 1
#             print(f"Accuracy so far: {correct_count}/{i+1}")
#         else:
#             if agent_name not in shortlisted_agents:
#                 not_shortlisted_count += 1
#                 print(
#                     f"Not shortlisted so far: {not_shortlisted_count}/{i+1 - correct_count}"
#                 )
#             print(f"   Query: {query}")
#             print(f"   Correct agent: {correct_agent_name}")
#             print(f"   Router selected agent: {agent_name}")
#             print(f"   Shortlisted agents: {shortlisted_agents}")

#         result = {
#             "query": query,
#             "correct_agent_name": correct_agent_name,
#             "routed_agent_name": agent_name,
#             "shortlisted_agents": shortlisted_agents,
#             "correct": agent_name == correct_agent_name,
#             "time_taken": elapsed,
#         }
#         results.append(result)

#         # save results to file
#         Path("router/results").mkdir(exist_ok=True)
#         with open("router/results/routing_results.json", "w") as f:
#             json.dump(results, f, indent=2)

#     avg_time = total_time / len([r for r in results if r["time_taken"]])
#     print(f"Saved results to router/results/routing_results.json")
#     print(f"Average response time: {avg_time:.3f}s")

#     accuracy = correct_count / total_queries * 100 if total_queries else 0
#     vec_search_failure = (
#         not_shortlisted_count / (total_queries - correct_count) * 100
#         if (total_queries - correct_count)
#         else 0
#     )
#     print(f"Correctly routed: {correct_count}/{total_queries} ({accuracy:.2f}%)")
#     print(
#         f"Not shortlisted: {not_shortlisted_count}/{total_queries - correct_count} ({vec_search_failure:.2f}%)"
#     )


if __name__ == "__main__":
    # agent_cards = load_agent_cards()
    # embeddings = OpenAIEmbeddings(
    #     model=settings.RERANKING_EMBEDDING_MODEL,
    #     openai_api_key=settings.OPENAI_API_KEY,
    # )
    # vectors = compute_agent_card_embeddings(agent_cards, embeddings)
    # vectors = np.array(vectors)
    # print("Saving agent card embeddings to agent_card_embeddings.npy")
    # np.save(EMBEDDINGS_FILE, vectors)
    # vectors = []
    # print(f"Vectors = {vectors}")
    test_router_quality(resume=False)
