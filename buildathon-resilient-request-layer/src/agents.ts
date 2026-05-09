export interface AgentPayload {
  input: string;
  userId?: string;
  workflowId?: string;
}

export interface AgentResult {
  agent_id: string;
  input: string;
  type: string;
  [key: string]: unknown;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function executeAgent(agentId: string, payload: AgentPayload): Promise<AgentResult> {
  switch (agentId) {
    case 'agent_fast': {
      await delay(100);
      return { agent_id: agentId, input: payload.input, type: 'fast' };
    }

    case 'agent_slow': {
      await delay(2000);
      return { agent_id: agentId, input: payload.input, type: 'slow' };
    }

    case 'agent_flaky': {
      const roll = Math.random();
      // 30% chance of transient failure
      if (roll < 0.3) {
        await delay(Math.random() * 300);
        throw new Error('agent_flaky: transient failure');
      }
      await delay(100 + Math.random() * 400);
      return { agent_id: agentId, input: payload.input, type: 'flaky', roll: Number(roll.toFixed(3)) };
    }

    default: {
      // Generic stub for any agent not explicitly defined above
      await delay(200);
      return { agent_id: agentId, input: payload.input, type: 'generic' };
    }
  }
}
