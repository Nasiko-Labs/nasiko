import axios from 'axios';

/**
 * DronaHQ Voice Agent Integration Service
 * 
 * This service handles communication with the DronaHQ Voice Agent API
 * to initiate outbound calls via NOVA.
 * 
 * CONFIGURATION:
 * - DRONAHQ_API_KEY: Your DronaHQ API key (set in .env)
 * - DRONAHQ_AGENT_ID: The NOVA Voice Agent ID (set in .env)
 */

const DRONAHQ_API_KEY = () => process.env.DRONAHQ_API_KEY;
const DRONAHQ_AGENT_ID = () => process.env.DRONAHQ_AGENT_ID || 'dcd6d607-ca16-4890-9582-f9c6b44890c2';
const DRONAHQ_API_HOST = () => process.env.DRONAHQ_API_HOST || 'https://studio.dronahq.com';

/**
 * Initiates an outbound call via DronaHQ NOVA Voice Agent.
 * 
 * @param {string} phoneNumber - The phone number to call (with country code, e.g. +919876543210)
 * @param {object} context - Lead context to personalize the call
 * @param {number} leadId - The lead ID for reference
 * @returns {object} Call initiation result with batch_id/status information
 */
export async function initiateCall(phoneNumber, context, leadId) {
  const apiKey = DRONAHQ_API_KEY();
  const agentId = DRONAHQ_AGENT_ID();
  const apiHost = DRONAHQ_API_HOST();

  if (!apiKey) {
    throw new Error('DRONAHQ_API_KEY is not configured. Please set it in your .env file.');
  }

  if (!agentId) {
    throw new Error('DRONAHQ_AGENT_ID is not configured. Please set it in your .env file.');
  }

  // Build personalized context message for NOVA
  const contextMessage = buildContextMessage(context);

  try {
    // Prevent double-appending if the user provided the full path in their env var
    const endpointUrl = apiHost.endsWith('/voice/outbound/dispatch')
      ? apiHost
      : `${apiHost}/voice/outbound/dispatch`;

    const response = await axios.post(
      endpointUrl,
      {
        agent_id: agentId,
        destination_phonenumber: phoneNumber,
        metadata: {
          lead_id: leadId,
          context: contextMessage,
          source: 'nova-crm'
        }
      },
      {
        headers: {
          'api-key': apiKey,
          'Content-Type': 'application/json'
        },
        timeout: 30000
      }
    );

    console.log('📞 DronaHQ call dispatched:', response.data);

    return {
      success: true,
      call_id: response.data?.batch_id || response.data?.id || `nova-batch-${Date.now()}`,
      status: response.data?.status || 'dispatched',
      batch_id: response.data?.batch_id,
      data: response.data
    };
  } catch (error) {
    let errorDetails = error.message;

    if (error.response) {
      const status = error.response.status;
      const responseData = error.response.data;
      
      // Prevent raw HTML pages from bleeding into the UI error alert
      let apiMsg = '';
      if (typeof responseData === 'string' && responseData.includes('<html')) {
        apiMsg = 'The provided DronaHQ API Host or endpoint path is unreachable.';
      } else {
        apiMsg = responseData?.message || responseData?.error || JSON.stringify(responseData);
      }
      
      if (status === 400) {
        errorDetails = `Bad Request (400) - Invalid parameters or missing E.164 phone number. ${apiMsg}`;
      } else if (status === 401) {
        errorDetails = `Unauthorized (401) - Invalid api-key header. ${apiMsg}`;
      } else if (status === 403) {
        errorDetails = `Forbidden (403) - Missing voice agent scope permissions. ${apiMsg}`;
      } else if (status === 404) {
        errorDetails = `Not Found (404) - Invalid endpoint or missing agent_id. ${apiMsg}`;
      } else {
        errorDetails = `API Error (${status}) - ${apiMsg}`;
      }
    }
    
    console.error('❌ DronaHQ API call failed:', errorDetails);
    throw new Error(errorDetails);
  }
}

/**
 * Builds a personalized context message for NOVA from lead data.
 */
function buildContextMessage(context) {
  const parts = [];

  if (context.lead_name) parts.push(`You are calling ${context.lead_name}`);
  if (context.company) parts.push(`from ${context.company}`);
  if (context.job_title) parts.push(`who is a ${context.job_title}`);
  if (context.industry) parts.push(`in the ${context.industry} industry`);
  if (context.customer_requirement) parts.push(`They are interested in: ${context.customer_requirement}`);
  if (context.pain_point) parts.push(`Their main challenge is: ${context.pain_point}`);
  if (context.notes) parts.push(`Additional notes: ${context.notes}`);

  return parts.join('. ') + '.';
}
