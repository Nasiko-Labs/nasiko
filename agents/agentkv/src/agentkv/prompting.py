"""Stable, information-preserving prompts for coding and support pilots."""
CODING_SYSTEM = ("You are a careful Python engineer. Follow the task contract. "
                 "Do not import modules. Follow the requested output format.")
SUPPORT_SYSTEM = ("You are a careful support operator. Follow the knowledge base. "
                  "Do not invent policy. Follow the requested output format.")


def shared_context(task: dict) -> str:
    if task.get("kind") == "support":
        return "Knowledge base:\n" + task["context"]
    return "Repository source and contracts:\n" + task["context"]


def instruction(task: dict, role: str = "coder", code: str = "") -> str:
    if task.get("kind") == "support":
        ticket = "Ticket: " + task["goal"]
        if role == "triage":
            return ("Classify this ticket as billing, shipping, or access. "
                    "Return only one of those labels.\n" + ticket)
        if role == "drafter":
            return ("Write a three-sentence reply that cites the matching policy clause. "
                    "Do not invent compensation.\n" + ticket + "\nClass: " + code)
        raise ValueError("unsupported support role")
    text = "Task: repair " + task["id"] + ". " + task["goal"]
    if role == "coder":
        text += " Return only the requested standalone function in a Python code block."
    if role == "reviewer":
        text += "\nReview the proposed function. Return PASS or FAIL and one concise reason.\n" + code
    return text


def compile_prompt(task: dict, method: str, role: str = "coder", code: str = ""):
    """A keeps task-first user text. B–E put the same shared context in the system prefix."""
    context = shared_context(task)
    task_text = instruction(task, role, code)
    system = SUPPORT_SYSTEM if task.get("kind") == "support" else CODING_SYSTEM
    if method == "A":
        return [{"role": "system", "content": system},
                {"role": "user", "content": task_text + "\n\n" + context}]
    return [{"role": "system", "content": system + "\n\n" + context},
            {"role": "user", "content": task_text}]
