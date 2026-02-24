# pytest routing_big_tests.py -v -s (-s to see the output on terminal)
import json
import os
import random
import time
from datetime import datetime
from pathlib import Path
from typing import List, Dict, Any

import numpy as np
from tqdm import tqdm
import faiss
from langchain_core.embeddings import Embeddings
from langchain_openai import OpenAIEmbeddings
from langchain_community.vectorstores import FAISS
from langchain.schema import Document
from langchain_community.vectorstores.faiss import InMemoryDocstore


from router.src.core.routing_engine import RoutingEngine
from router.src.config import settings


AGENT_CARDS_DIR = "router/data/agent_cards"
# AGENT_CARDS_DIR = "router/data/maf_agent_cards"
QUERIES_RESPONSES_DIR = "router/data/query_response_pairs"
REGISTRIES_FILE = "router/data/registries.json"
# REGISTRIES_FILE = "router/data/maf_registries.json"
# TEST_CASES_FILE = "router/data/test_cases.json"
TEST_CASES_FILE = "router/data/test_cases_long.json"
# TEST_CASES_FILE = "router/data/mt_test_cases.json"
EMBEDDINGS_FILE = "router/data/agent_card_embeddings.npy"
# EMBEDDINGS_FILE = "router/data/mt_agent_card_embeddings.npy"
SAMPLED_REGISTRIES_FILE = "router/data/sampled_registries.json"
# SAMPLED_REGISTRIES_FILE = "router/data/mt_sampled_registries.json"
PROCESSED_CASES_FILE = "router/data/processed_cases.json"
RESULTS_DIR = "router/results"
FAILURES_DIR = "router/results/failures"
RESULTS_FILE = "router/results/results.txt"
SHORTLISTS_FILE = "router/results/shortlists.txt"

# Fixed seed for reproducibility
RANDOM_SEED = 65

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
    ("20-70", lambda s: 36 <= s <= 70),
    # ("2-35", lambda s: 2 <= s <= 35),
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
        # indices_in_range = [
        #     i for i, reg in enumerate(registries) if range_filter(reg["size"])
        # ]

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


def semantic_search_exps(embedding_file=EMBEDDINGS_FILE):
    # Load registries.
    registries = load_registries()

    # Load test cases.
    test_cases = load_test_cases()

    # Select 10% of registries for each size range.
    selected_registry_indices = select_registries_by_size_ranges(
        registries, sample_fraction=0.05
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
        # TODO: Change embeddings model to that used in semantic search.
        agent_cards_embeddings = compute_agent_card_embeddings(
            agent_cards,
            OpenAIEmbeddings(
                model=settings.RERANKING_EMBEDDING_MODEL,
                openai_api_key=settings.OPENAI_API_KEY,
            ),
        )
        np.save(EMBEDDINGS_FILE, agent_cards_embeddings)

    # Load agent card embeddings.
    print("Loading agent card embeddings from agent_card_embeddings.npy")
    agent_cards_embeddings = np.load(EMBEDDINGS_FILE)
    print(
        f"Loaded {len(agent_cards_embeddings)} embeddings from agent_card_embeddings.npy"
    )

    # Create embeddings model for vectorstore
    embeddings_model = OpenAIEmbeddings(
        model=settings.RERANKING_EMBEDDING_MODEL,
        openai_api_key=settings.OPENAI_API_KEY,
    )

    # Create output directories
    Path(RESULTS_DIR).mkdir(parents=True, exist_ok=True)

    # Initialize tracking variables for shortlist statistics
    total_turns = 0
    failed_turns = 0
    failed_not_in_first_list = 0
    failed_not_in_second_list = 0
    failed_in_first_not_in_second = 0

    # Initialize routing engine
    router = RoutingEngine()

    # Open shortlists file for writing
    shortlists_file = open(SHORTLISTS_FILE, "w", encoding="utf-8")
    shortlists_file.write("=" * 80 + "\n")
    shortlists_file.write("SHORTLISTS FOR ALL TURNS\n")
    shortlists_file.write("=" * 80 + "\n\n")

    # Process each registry
    for registry_idx in tqdm(selected_registry_indices, desc="Processing registries"):
        registry = registries[registry_idx]
        # registry_size = registry["size"]
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

        # Write registry header to shortlists file
        shortlists_file.write("-" * 80 + "\n")
        shortlists_file.write(
            f"REGISTRY {registry_idx} (size: {registry_size}, range: {size_range})\n"
        )
        shortlists_file.write("-" * 80 + "\n\n")

        # Process each test case (conversation)
        for test_case_idx, test_case in enumerate(registry_test_cases):
            print(f"Processing test case {test_case_idx} for registry {registry_idx}.")
            conversation_history = []
            turn_times = []  # Track time for each turn

            shortlists_file.write(f"  Test Case {test_case_idx}:\n")

            for turn_idx, query in enumerate(test_case["queries"]):
                print(f"  Turn {turn_idx}: {query}.")
                turn_data = load_turn_data(query, registry)
                correct_agent = turn_data["agent_name"]
                total_turns += 1

                # Call the router
                try:
                    turn_start_time = time.time()
                    message = turn_data["Human Message"]
                    (
                        first_shortlist,
                        similarity_scores,
                        second_shortlist,
                        router_output,
                    ) = router.route_query(
                        message=message,
                        conversation_history=conversation_history,
                        agent_cards=registry_agent_cards,
                        vectorstore=vectorstore,
                    )
                    turn_elapsed_time = time.time() - turn_start_time
                    turn_times.append(turn_elapsed_time)

                    selected_agent = router_output.agent_name
                    turn_failed = selected_agent != correct_agent

                    # Write shortlists to file
                    shortlists_file.write(f"    Turn {turn_idx}:\n")
                    shortlists_file.write(f"      User Message: {message}\n")
                    shortlists_file.write(f"      Correct Agent: {correct_agent}\n")
                    shortlists_file.write(f"      First Shortlist: {first_shortlist}\n")
                    shortlists_file.write(
                        f"      Similarity Scores: {similarity_scores}\n"
                    )
                    shortlists_file.write(
                        f"      Second Shortlist: {second_shortlist}\n"
                    )
                    shortlists_file.flush()
                    if turn_failed:
                        shortlists_file.write(
                            f"      STATUS: FAILED (selected: {selected_agent})\n"
                        )
                    else:
                        shortlists_file.write(f"      STATUS: PASSED\n")
                    shortlists_file.write("\n")

                    # Track shortlist statistics for failed turns
                    if turn_failed:
                        failed_turns += 1
                        in_first = correct_agent in first_shortlist
                        in_second = correct_agent in second_shortlist

                        if not in_first:
                            failed_not_in_first_list += 1
                        if not in_second:
                            failed_not_in_second_list += 1
                        if in_first and not in_second:
                            failed_in_first_not_in_second += 1

                except Exception as e:
                    print(
                        f"Error routing turn {turn_idx} in registry {registry_idx}: {e}"
                    )
                    failed_turns += 1

                    # Write error to shortlists file
                    shortlists_file.write(f"    Turn {turn_idx}:\n")
                    shortlists_file.write(f"      Correct Agent: {correct_agent}\n")
                    shortlists_file.write(f"      STATUS: FAILED (error: {e})\n")
                    shortlists_file.write("\n")

                # Append turn to conversation history
                conversation_history.append(
                    {"role": "Human", "content": turn_data["Human Message"]}
                )
                conversation_history.append(
                    {"role": "Assistant", "content": turn_data["AI Message"]}
                )

            # Write average time per turn for this conversation
            if turn_times:
                avg_time_per_turn = sum(turn_times) / len(turn_times)
                shortlists_file.write(
                    f"    Average time per turn: {avg_time_per_turn:.3f}s\n"
                )
            shortlists_file.write("\n")

        shortlists_file.write("\n")

    # Close shortlists file
    shortlists_file.close()

    # Print shortlist statistics
    print(f"\nShortlists written to {SHORTLISTS_FILE}")
    print("\n" + "=" * 60)
    print("SHORTLIST STATISTICS FOR FAILED TURNS")
    print("=" * 60)
    if failed_turns > 0:
        print(
            f"Total failed turns: {failed_turns} / {total_turns} ({failed_turns / total_turns * 100:.2f}%)"
        )
        print(
            f"Failed turns where correct agent NOT in first shortlist: {failed_not_in_first_list}/{failed_turns} ({failed_not_in_first_list / failed_turns * 100:.2f}%)"
        )
        print(
            f"Failed turns where correct agent NOT in second shortlist: {failed_not_in_second_list}/{failed_turns} ({failed_not_in_second_list / failed_turns * 100:.2f}%)"
        )
        print(
            f"Failed turns where correct agent in first but NOT in second shortlist: {failed_in_first_not_in_second}/{failed_turns} ({failed_in_first_not_in_second / failed_turns * 100:.2f}%)"
        )
    else:
        print("No failed turns.")

    # Write shortlist statistics to timestamped file
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    results_filename = f"semantic_search_results_{timestamp}.txt"
    results_filepath = Path(RESULTS_DIR) / results_filename
    with open(results_filepath, "w", encoding="utf-8") as f:
        f.write("=" * 60 + "\n")
        f.write("SHORTLIST STATISTICS FOR FAILED TURNS\n")
        f.write("=" * 60 + "\n")
        f.write(f"Total turns: {total_turns}\n")
        f.write(
            f"Failed turns: {failed_turns}/{total_turns} ({failed_turns / total_turns * 100:.2f}%)\n"
        )
        if failed_turns > 0:
            f.write(
                f"Failed turns where correct agent NOT in first shortlist: {failed_not_in_first_list}/{failed_turns} ({failed_not_in_first_list / failed_turns * 100:.2f}%)\n"
            )
            f.write(
                f"Failed turns where correct agent NOT in second shortlist: {failed_not_in_second_list}/{failed_turns} ({failed_not_in_second_list / failed_turns * 100:.2f}%)\n"
            )
            f.write(
                f"Failed turns where correct agent in first but NOT in second shortlist: {failed_in_first_not_in_second}/{failed_turns} ({failed_in_first_not_in_second / failed_turns * 100:.2f}%)\n"
            )
    print(f"\nShortlist statistics written to {results_filepath}")

    return {
        "total_turns": total_turns,
        "failed_turns": failed_turns,
        "failed_not_in_first_list": failed_not_in_first_list,
        "failed_not_in_second_list": failed_not_in_second_list,
        "failed_in_first_not_in_second": failed_in_first_not_in_second,
    }


if __name__ == "__main__":
    # test_routing(total_queries=100)
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
    # vectors = np.load(EMBEDDINGS_FILE)
    # print(f"Loaded {len(vectors)} many vectors from {EMBEDDINGS_FILE}")
    semantic_search_exps()
