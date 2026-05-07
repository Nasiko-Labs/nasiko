{
  "protocolVersion": "0.2.9",
  "name": "MCP Calculator Agent",
  "description": "A calculator agent that performs math operations — add, subtract, multiply, divide. Built for the Nasiko MCP Hackathon to demonstrate agent upload, deployment, and chat.",
  "url": "http://localhost:5000/",
  "preferredTransport": "JSONRPC",
  "provider": {
    "organization": "Nasiko MCP Hackathon",
    "url": "https://github.com/Nasiko-Labs/nasiko"
  },
  "version": "1.0.0",
  "capabilities": {
    "streaming": false,
    "pushNotifications": false,
    "stateTransitionHistory": false,
    "chat_agent": false
  },
  "defaultInputModes": ["text/plain"],
  "defaultOutputModes": ["text/plain"],
  "skills": [
    {
      "id": "add",
      "name": "Add Numbers",
      "description": "Add two numbers together",
      "tags": ["math", "calculator", "addition"],
      "examples": ["Add 40 and 2", "What is 100 + 200?"]
    },
    {
      "id": "subtract",
      "name": "Subtract Numbers",
      "description": "Subtract one number from another",
      "tags": ["math", "calculator", "subtraction"],
      "examples": ["Subtract 10 from 50", "What is 100 - 25?"]
    },
    {
      "id": "multiply",
      "name": "Multiply Numbers",
      "description": "Multiply two numbers together",
      "tags": ["math", "calculator", "multiplication"],
      "examples": ["Multiply 6 by 7", "What is 12 times 5?"]
    },
    {
      "id": "divide",
      "name": "Divide Numbers",
      "description": "Divide one number by another",
      "tags": ["math", "calculator", "division"],
      "examples": ["Divide 100 by 4", "What is 50 / 10?"]
    }
  ],
  "supportsAuthenticatedExtendedCard": false,
  "signatures": []
}
