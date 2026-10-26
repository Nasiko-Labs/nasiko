# Two trivial LangGraph graphs; `langgraph dev` serves an A2A card for each.
from langgraph.graph import END, START, MessagesState, StateGraph


def reply(state: MessagesState):
    return {"messages": [("ai", "ok")]}


def graph():
    builder = StateGraph(MessagesState)
    builder.add_node("reply", reply)
    builder.add_edge(START, "reply")
    builder.add_edge("reply", END)
    return builder.compile()


planner = graph()
writer = graph()
