# A2A Translator Agent

An intelligent translation agent built with A2A (Agent2Agent) SDK that can translate text and web page content using natural language.

## Features

- **Text Translation**: Translate plain text between different languages
- **URL Content Translation**: Extract and translate content from web pages
- **Language Detection**: Automatically detect the language of text or web content
- **Multi-language Support**: Supports translation between multiple languages using Google Translate
- **Clean Web Content**: Extracts readable text from HTML pages, removing scripts and styling

## Installation

1. Clone the repository
2. Install dependencies:
   ```bash
   pip install -e .
   ```

3. Set up environment variables:
   ```bash
   export OPENAI_API_KEY="your-openai-api-key"
   ```

## Usage

### Running the Agent

```bash
# Run locally
python -m src --host localhost --port 5000

# Or using Docker
docker-compose up
```

### Available Functions

#### Translate Text
Translate plain text from one language to another:
- Auto-detects source language if not specified
- Supports all languages supported by Google Translate

#### Translate URL Content
Extract and translate content from web pages:
- Extracts clean, readable text from HTML
- Handles various webpage formats
- Provides page title information
- Limits content to manageable chunks (5000 characters)

#### Detect Language
Detect the language of given text or URL content:
- Returns language code and confidence level
- Works with both direct text and web content
- Provides text sample for verification

## Examples

### Text Translation
```
"Translate 'Hello world' to Spanish"
"Convert this text to French: 'Good morning'"
```

### URL Translation
```
"Translate the content of https://example.com to German"
"What does this Spanish website say in English?"
```

### Language Detection
```
"What language is this text written in?"
"Detect the language of this webpage"
```

## Configuration

The agent runs on port 5000 by default. You can customize the host and port using command line options:

```bash
python -m src --host 0.0.0.0 --port 8080
```

## Dependencies

- a2a-sdk: A2A framework for agent communication
- googletrans: Google Translate API wrapper
- beautifulsoup4: HTML parsing for web content extraction
- langdetect: Language detection library
- requests: HTTP client for web scraping
- OpenAI: For agent conversation handling

## Environment Variables

- `OPENAI_API_KEY`: Required for agent conversation handling
- `PORT`: Server port (optional, default: 5000)
- `HOST`: Server host (optional, default: localhost)