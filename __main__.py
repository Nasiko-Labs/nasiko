version: '3.8'
services:
  gateway-agent:
    build: .
    environment:
      - OPENAI_API_BASE=http://llm-gateway:4000
      - OPENAI_API_KEY=nasiko-virtual-proxy-key
