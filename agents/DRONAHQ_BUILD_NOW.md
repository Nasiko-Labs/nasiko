# DronaHQ — build Grower now

Use Veda AI to generate the first UI scaffold. DronaHQ supports prompt-based screen generation and actionflow generation, then use normal controls/data bindings to finish the behavior.

## Veda prompt

Create a polished single-page SaaS app called Grower.

Purpose: turn a product idea or website into a GTM campaign.

Screen states:
1. Landing: "Grower" / "Turn your idea into customers." with two buttons: "Yes, I have a website" and "I don't have a website".
2. Website form: URL input and Grow button.
3. No-website form: three inputs:
   - What do you sell or want to build?
   - Who is your ideal customer?
   - Which markets or geography?
   Button: "Generate my campaign →"
4. Agent progress: four cards with Research Agent, ICP Agent, Prospect Agent, Outreach Agent and statuses.
5. Campaign result: ICP, buying signals, three prospect cards, and personalized outreach cards with "Book a demo".

Style: clean dark SaaS dashboard, generous whitespace, rounded cards, subtle borders, strong typography, one primary accent. Desktop-first but responsive.

Create controls with unique names:
website_mode, website_url, product_input, icp_input, geography_input, generate_button, progress_container, results_container.

Do not create a database.

## API action

Create an action on generate_button using JS Code. Use:

const payload = {
  website: {{website_url}},
  product: {{product_input}},
  ideal_customer: {{icp_input}},
  geography: {{geography_input}}
};

const body = "GROW_CAMPAIGN: " + JSON.stringify(payload);

const response = await UTILITY.CALLRESTAPI(
  "https://YOUR_ASSISTANT_PUBLIC_URL/",
  "POST",
  {"Content-Type":"application/json","A2A-Version":"1.0"},
  {
    "jsonrpc":"2.0",
    "id": String(Date.now()),
    "method":"SendMessage",
    "params":{
      "message":{
        "messageId":String(Date.now()),
        "role":"ROLE_USER",
        "parts":[{"text":body}]
      }
    }
  },
  true,
  180000
);

Set the returned response into a hidden/state control named campaign_response, then render the JSON into the result controls.

## No website behavior

When the user clicks "I don't have a website", show the three business inputs and hide website_url.

When "Yes, I have a website" is clicked, show website_url and hide product/icp/geography.

For the hackathon demo, use:
Product = AI voice agents for B2B SaaS companies
ICP = Founders and VP Sales
Geography = US and Europe

## Important

Do not put ANAKIN_API_KEY or OPENAI_API_KEY in DronaHQ. Those stay on the Nasiko/agent runtime.

The assistant response has this shape:

{
  "type": "grower_campaign",
  "stages": [
    {"agent":"grower-research-agent","response":"..."},
    {"agent":"grower-icp-agent","response":"..."},
    {"agent":"grower-prospect-agent","response":"..."},
    {"agent":"grower-outreach-agent","response":"..."}
  ]
}

Each response string is JSON from the specialist agent. Parse it before displaying the final cards.
