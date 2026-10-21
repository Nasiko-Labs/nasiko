# Grower DronaHQ UI Spec

Build this as a 4-state app.

## 1. Landing

Title: Grower
Subtitle: Turn your idea into customers.

Question: Do you have a website?

Buttons:
- Yes, I have a website
- I don't have a website

## 2A. Website form

Field: Website URL
Button: Grow

On submit create:

GROW_CAMPAIGN: {"website":"<url>","product":"","ideal_customer":"","geography":[]}

## 2B. No-website form

Fields:
- What do you sell or want to build?
- Who is your ideal customer?
- Which markets or geography?

Button: Generate my campaign

On submit create:

GROW_CAMPAIGN: {"website":"","product":"<product>","ideal_customer":"<icp>","geography":"<geography>"}

## 3. Agent execution

Show four steps as progress cards:

1. Research Agent — Researching market
2. ICP Agent — Sharpening ICP
3. Prospect Agent — Finding high-signal prospects
4. Outreach Agent — Writing personalized outreach

The UI can update these states optimistically while the A2A request runs, then render the returned JSON.

## 4. Campaign result

Sections:
- ICP
- Buying Signals
- Hot Prospects
- Personalized Outreach

For each prospect show:
company, signal, heuristic score, reason, source.

For outreach show:
company, subject, email, CTA = Book a demo.

## API

Use a DronaHQ REST/API action against the Nasiko Assistant A2A endpoint.

POST JSON-RPC:

{
  "jsonrpc": "2.0",
  "id": "<uuid>",
  "method": "SendMessage",
  "params": {
    "message": {
      "messageId": "<uuid>",
      "role": "ROLE_USER",
      "parts": [
        {"text": "GROW_CAMPAIGN: <campaign-json>"}
      ]
    }
  }
}

Required runtime configuration:
- Assistant URL
- A2A discovery URL
- ANAKIN_API_KEY
- OPENAI_API_KEY

Do not put secrets into the DronaHQ UI. Keep them in the server/agent environment.
