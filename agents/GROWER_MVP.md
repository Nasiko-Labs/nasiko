# Grower GTM MVP

Grower turns a product idea or website into a small outbound campaign.

## Flow

DronaHQ UI -> Nasiko Assistant -> A2A specialist agents -> result

1. Research Agent: Anakin web research + website scrape
2. ICP Agent: ICP + observable buying signals
3. Prospect Agent: live prospect discovery with Anakin
4. Outreach Agent: personalized email drafts

No email is sent in the MVP. The final CTA is a demo link.

## No-website input

Send this JSON as the campaign payload:

```json
{
  "website": "",
  "product": "AI voice agents for B2B SaaS companies",
  "ideal_customer": "Founders and VP Sales",
  "geography": ["US", "Europe"]
}
```

Prefix the A2A assistant user message with `GROW_CAMPAIGN:`.

Example:

`GROW_CAMPAIGN: {"website":"","product":"AI voice agents for B2B SaaS companies","ideal_customer":"Founders and VP Sales","geography":["US","Europe"]}`

The Assistant discovers the four registered Grower agents and passes each stage's output to the next stage.

## Environment

Set on the agents:

- OPENAI_API_KEY
- OPENAI_BASE_URL (optional)
- MODEL (optional; defaults to deepseek-v4-flash)
- ANAKIN_API_KEY

Set on the Assistant:

- A2A_DISCOVERY_URL
- OPENAI_API_KEY
- OPENAI_BASE_URL (optional)
- MODEL (optional)

## Hackathon demo

Use the no-website path:

Product: AI voice agents for B2B SaaS companies
ICP: Founders and VP Sales
Geography: US and Europe

Show:

Campaign input -> Research -> ICP -> Prospects -> Outreach -> Book Demo CTA
