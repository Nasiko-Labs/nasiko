{
  "protocolVersion": "0.2.9",
  "name": "MCP Calculator Server",
  "description": "A Model Context Protocol (MCP) calculator server that provides mathematical operations as tools. Built for the Nasiko MCP Hackathon to demonstrate MCP server upload, detection, and deployment.",
  "url": "http://localhost:5000/",
  "preferredTransport": "STDIO",
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
  "defaultInputModes": ["application/json"],
  "defaultOutputModes": ["application/json"],
  "skills": [
    {
      "id": "add",
      "name": "Add Numbers",
      "description": "Add two numbers together and return the result",
      "tags": ["math", "calculator", "addition"],
      "examples": ["Add 40 and 2", "What is 100 + 200?"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "multiply",
      "name": "Multiply Numbers",
      "description": "Multiply two numbers and return the product",
      "tags": ["math", "calculator", "multiplication"],
      "examples": ["Multiply 6 by 7", "What is 12 times 5?"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    },
    {
      "id": "divide",
      "name": "Divide Numbers",
      "description": "Divide one number by another with division-by-zero protection",
      "tags": ["math", "calculator", "division"],
      "examples": ["Divide 100 by 4", "What is 50 / 10?"],
      "inputModes": ["application/json"],
      "outputModes": ["application/json"]
    }
  ],
  "supportsAuthenticatedExtendedCard": false,
  "signatures": []
}
