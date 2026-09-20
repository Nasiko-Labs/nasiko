const express = require('express');
const axios = require('axios');
const fs = require('fs');
const crypto = require('crypto');

const app = express();
app.use(express.json());

const PORT = process.env.PORT || 8000;
const DRONAHQ_WEBHOOK_URL = process.env.DRONAHQ_WEBHOOK_URL;

// Serve the AgentCard so Nasiko can discover our skills
app.get('/.well-known/agent-card.json', (req, res) => {
    try {
        const card = JSON.parse(fs.readFileSync('./AgentCard.json', 'utf8'));
        res.json(card);
    } catch (e) {
        res.status(500).json({ error: "Failed to read AgentCard.json" });
    }
});

// Main A2A JSON-RPC interface
app.post('/a2a', async (req, res) => {
    const { jsonrpc, id, method, params } = req.body;

    if (jsonrpc !== '2.0') {
        return res.status(400).json({
            jsonrpc: "2.0",
            id: id || null,
            error: { code: -32600, message: "Invalid Request" }
        });
    }
    
    console.log(`[A2A Finance Agent] Received method: ${method}`);

    try {
        // 1. Extract the text from the A2A message
        const messageParts = params?.message?.parts || [];
        const textParts = messageParts.filter(p => p.text).map(p => p.text);
        const userQuery = textParts.join('\n');

        console.log(`[A2A Finance Agent] Received query: "${userQuery}"`);

        // 2. Call the DronaHQ webhook
        if (!DRONAHQ_WEBHOOK_URL) {
            throw new Error("DRONAHQ_WEBHOOK_URL is not set in environment");
        }

        console.log(`[A2A Finance Agent] Forwarding to DronaHQ: ${DRONAHQ_WEBHOOK_URL}`);
        
        // We send the user query to DronaHQ. (Adjust the payload key 'query' if your DronaHQ webhook expects a different field name).
        const dronaResponse = await axios.post(DRONAHQ_WEBHOOK_URL, {
            query: userQuery
        });

        // Extract DronaHQ's text answer.
        let botAnswer = "";
        if (typeof dronaResponse.data === 'string') {
            botAnswer = dronaResponse.data;
        } else if (dronaResponse.data.answer) {
            botAnswer = dronaResponse.data.answer;
        } else if (dronaResponse.data.response) {
            botAnswer = dronaResponse.data.response;
        } else {
            botAnswer = JSON.stringify(dronaResponse.data, null, 2);
        }

        console.log(`[A2A Finance Agent] Received response from DronaHQ (length: ${botAnswer.length})`);

        // 3. Format the response back to Nasiko A2A spec
        const responsePayload = {
            jsonrpc: "2.0",
            id: id,
            result: {
                task: {
                    id: `task-${crypto.randomUUID()}`,
                    contextId: params?.message?.contextId || `ctx-${crypto.randomUUID()}`,
                    status: {
                        state: "TASK_STATE_COMPLETED",
                        timestamp: new Date().toISOString()
                    },
                    artifacts: [
                        {
                            artifactId: `art-${crypto.randomUUID()}`,
                            parts: [
                                { text: botAnswer }
                            ]
                        }
                    ]
                }
            }
        };

        res.json(responsePayload);

    } catch (error) {
        console.error("[A2A Error]", error.message);
        
        res.json({
            jsonrpc: "2.0",
            id: id,
            result: {
                task: {
                    id: `task-${crypto.randomUUID()}`,
                    contextId: params?.message?.contextId || `ctx-error`,
                    status: {
                        state: "TASK_STATE_FAILED",
                        message: {
                            role: "ROLE_SYSTEM",
                            parts: [{ text: `Finance Bridge Agent Error: ${error.message}` }]
                        },
                        timestamp: new Date().toISOString()
                    }
                }
            }
        });
    }
});

// Healthcheck endpoint
app.get('/', (req, res) => res.send('OK'));

app.listen(PORT, () => {
    console.log(`Finance Agent listening on port ${PORT}`);
    if (!DRONAHQ_WEBHOOK_URL) {
        console.warn("WARNING: DRONAHQ_WEBHOOK_URL is missing. API calls will fail.");
    }
});
