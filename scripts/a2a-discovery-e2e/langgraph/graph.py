# Two LangGraph graphs; `langgraph dev` serves an A2A card for each. `planner` calls an
# OpenAI model, so the test can check its calls reach Nasiko once `nasiko connect` points it there.
from langchain_openai import ChatOpenAI
from langgraph.graph import END, START, MessagesState, StateGraph


def ask_model(state: MessagesState):
    return {"messages": [ChatOpenAI(model="gpt-4o-mini").invoke(state["messages"])]}


def reply(state: MessagesState):
    return {"messages": [("ai", "ok")]}


def graph(node):
    builder = StateGraph(MessagesState)
    builder.add_node("node", node)
    builder.add_edge(START, "node")
    builder.add_edge("node", END)
    return builder.compile()


planner = graph(ask_model)
writer = graph(reply)
