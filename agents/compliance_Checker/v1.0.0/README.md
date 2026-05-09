# Compliance Checker Agent (A2A Compatible)

An A2A-compatible agent that analyzes documents for policy violations and compliance issues.

## Features

- Document compliance checking against organizational policies
- Policy analysis and explanations
- Specific violation identification with evidence
- Recommendations for fixing compliance issues

## Policies Checked

1. **Professional Tone** - All communications must maintain respectful, professional tone
2. **No Sensitive Data Sharing** - Must not include PII unless encrypted/authorized
3. **IFRS vs. GAAP** - Financial reports must follow IFRS, not GAAP
4. **Expense Approvals** - Expenses over $50,000 require CEO/finance approval
5. **Encryption** - File transfers must specify encryption method
6. **Work Hours** - No work outside 9am-6pm without approval
7. **Internal Communication** - No inter-department document sharing

## Setup

1. Install dependencies:
```bash
pip install -e .
```

2. Set up environment variables:
```bash
export OPENROUTER_API_KEY="your-api-key"
export MONGO_URL="mongodb://localhost:27017"
```

3. Ensure MongoDB is running for chat history storage.

## Running

```bash
python -m src --host localhost --port 10008
```

Options:
- `--host`: Host to bind to (default: localhost)
- `--port`: Port to bind to (default: 10008)
- `--mongo-url`: MongoDB connection URL (default: mongodb://localhost:27017)
- `--db-name`: Database name (default: compliance-checker-a2a)

## Usage

The agent exposes the following tools:

### check_compliance
Analyze a document for policy compliance.

**Parameters:**
- `document_text` (str): The document text to analyze
- `query` (str, optional): Specific question about compliance

### analyze_policy
Answer questions about specific policies.

**Parameters:**
- `policy_question` (str): Question about policies or compliance requirements

## Example Queries

- "Check this document for policy compliance"
- "Does this email violate any policies?"
- "What are the encryption requirements for file transfers?"
- "Analyze this expense report for compliance issues"