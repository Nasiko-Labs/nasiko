import { GoogleGenAI } from '@google/genai';
import { BedrockRuntimeClient, ConverseCommand } from '@aws-sdk/client-bedrock-runtime';
import { generateHeuristicFallback } from './heuristicFallback';

export interface LLMResponse<T = any> {
  data: T;
  rawText: string;
  latencyMs: number;
  provider: 'bedrock' | 'gemini' | 'openai' | 'fallback';
  model: string;
  estimatedTokens: {
    promptTokens: number;
    completionTokens: number;
    totalTokens: number;
  };
}

export interface ProviderStatus {
  activeProvider: 'bedrock' | 'gemini' | 'openai' | 'fallback';
  activeModel: string;
  bedrock: {
    configured: boolean;
    authType: 'api_key' | 'iam_keys' | 'pre_signed_url' | 'none';
    region: string;
    modelId: string;
    isPreSignedUrl: boolean;
  };
  gemini: {
    configured: boolean;
    models: string[];
  };
  openai: {
    configured: boolean;
    model: string;
  };
}

class LLMClient {
  private genAI: GoogleGenAI | null = null;
  private bedrockClient: BedrockRuntimeClient | null = null;
  private bedrockAuthErrorUntil: number = 0;
  private usePreSignedUrl: boolean = false;
  private preSignedUrl: string = '';
  private candidateGeminiModels = ['gemini-2.5-flash', 'gemini-2.0-flash', 'gemini-1.5-flash'];
  private candidateBedrockModels = [
    'amazon.nova-micro-v1:0',
    'amazon.nova-lite-v1:0',
    'meta.llama3-70b-instruct-v1:0',
    'meta.llama3-8b-instruct-v1:0',
  ];

  constructor() {
    this.initClients();
  }

  private initClients() {
    // 1. Initialize Gemini
    const geminiKey = process.env.GEMINI_API_KEY;
    if (geminiKey && geminiKey !== 'MY_GEMINI_API_KEY') {
      try {
        this.genAI = new GoogleGenAI({
          apiKey: geminiKey,
          httpOptions: {
            headers: {
              'User-Agent': 'aistudio-build',
            },
          },
        });
      } catch (err) {
        console.warn('[LLMClient] Failed to initialize Google GenAI:', err);
      }
    }

    // 2. Initialize Amazon Bedrock Client
    this.initBedrockClient();
  }

  private isPreSignedUrl(key: string): boolean {
    try {
      const decoded = Buffer.from(key, 'base64').toString('utf-8');
      return decoded.includes('bedrock.amazonaws.com') || decoded.includes('bedrock-runtime.');
    } catch { return false; }
  }

  private initBedrockClient() {
    const region = process.env.AWS_REGION || process.env.AWS_DEFAULT_REGION || 'us-east-1';
    const bedrockApiKey = process.env.BEDROCK_API_KEY;
    const accessKeyId = process.env.AWS_ACCESS_KEY_ID;
    const secretAccessKey = process.env.AWS_SECRET_ACCESS_KEY;
    const sessionToken = process.env.AWS_SESSION_TOKEN;

    this.usePreSignedUrl = false;
    this.preSignedUrl = '';

    try {
      // Priority 1: Pre-signed URL (base64 encoded)
      if (bedrockApiKey && bedrockApiKey.trim().length > 0 && this.isPreSignedUrl(bedrockApiKey)) {
        this.preSignedUrl = Buffer.from(bedrockApiKey, 'base64').toString('utf-8');
        this.usePreSignedUrl = true;
        this.bedrockClient = null; // We'll use fetch directly
        console.log(`[LLMClient] Amazon Bedrock initialized via pre-signed URL in region: ${region}`);
        return;
      }

      // Priority 2: Standard API Key / Bearer Token
      if (bedrockApiKey && bedrockApiKey.trim().length > 0) {
        if (bedrockApiKey.includes(':') && !accessKeyId) {
          const [keyId, secret] = bedrockApiKey.split(':');
          this.bedrockClient = new BedrockRuntimeClient({
            region,
            maxAttempts: 1,
            credentials: {
              accessKeyId: keyId.trim(),
              secretAccessKey: secret.trim(),
            },
          });
          console.log(`[LLMClient] Amazon Bedrock initialized via composite API key credentials in region: ${region}`);
          return;
        }

        // Bearer Token mode
        this.bedrockClient = new BedrockRuntimeClient({
          region,
          maxAttempts: 1,
          token: { token: bedrockApiKey.trim() },
        });
        console.log(`[LLMClient] Amazon Bedrock initialized via direct API Bearer Token in region: ${region}`);
        return;
      }

      // Priority 3: Standard AWS IAM credentials
      if (accessKeyId && secretAccessKey) {
        this.bedrockClient = new BedrockRuntimeClient({
          region,
          maxAttempts: 1,
          credentials: {
            accessKeyId: accessKeyId.trim(),
            secretAccessKey: secretAccessKey.trim(),
            sessionToken: sessionToken ? sessionToken.trim() : undefined,
          },
        });
        console.log(`[LLMClient] Amazon Bedrock initialized via standard AWS IAM credentials in region: ${region}`);
      }
    } catch (err: any) {
      console.warn('[LLMClient] Bedrock initialization error:', err?.message || err);
      this.bedrockClient = null;
    }
  }

  getProviderStatus(): ProviderStatus {
    const region = process.env.AWS_REGION || process.env.AWS_DEFAULT_REGION || 'us-east-1';
    const bedrockApiKey = process.env.BEDROCK_API_KEY;
    const isPreSignedUrl = this.isPreSignedUrl(bedrockApiKey || '');

    const bedrockConfigured = Boolean(
      (bedrockApiKey && bedrockApiKey.trim().length > 0) ||
      (process.env.AWS_ACCESS_KEY_ID && process.env.AWS_SECRET_ACCESS_KEY)
    );

    const bedrockAuthType = isPreSignedUrl
      ? 'pre_signed_url'
      : bedrockApiKey
        ? 'api_key'
        : process.env.AWS_ACCESS_KEY_ID
          ? 'iam_keys'
          : 'none';

    const geminiConfigured = Boolean(
      process.env.GEMINI_API_KEY && process.env.GEMINI_API_KEY !== 'MY_GEMINI_API_KEY'
    );

    const openaiConfigured = Boolean(process.env.OPENAI_API_KEY);

    const preferredBedrockModel =
      process.env.BEDROCK_MODEL_ID || 'amazon.nova-micro-v1:0';

    let activeProvider: 'bedrock' | 'gemini' | 'openai' | 'fallback' = 'fallback';
    let activeModel = 'Heuristic Local Synthesis Engine';

    if (bedrockConfigured) {
      activeProvider = 'bedrock';
      activeModel = preferredBedrockModel;
    } else if (geminiConfigured) {
      activeProvider = 'gemini';
      activeModel = 'gemini-2.5-flash';
    } else if (openaiConfigured) {
      activeProvider = 'openai';
      activeModel = 'gpt-4o-mini';
    }

    return {
      activeProvider,
      activeModel,
      bedrock: {
        configured: bedrockConfigured,
        authType: bedrockAuthType,
        region,
        modelId: preferredBedrockModel,
        isPreSignedUrl,
      },
      gemini: {
        configured: geminiConfigured,
        models: this.candidateGeminiModels,
      },
      openai: {
        configured: openaiConfigured,
        model: 'gpt-4o-mini',
      },
    };
  }

  async generateStructuredJSON<T = any>(
    systemInstruction: string,
    prompt: string,
    temperature = 0.2
  ): Promise<LLMResponse<T>> {
    const startTime = Date.now();

    // 1. Pre-signed URL Bedrock (highest priority if configured)
    if (this.usePreSignedUrl) {
      try {
        return await this.callBedrockViaPreSignedUrl<T>(systemInstruction, prompt, temperature, startTime);
      } catch (err: any) {
        console.warn('[LLMClient] Pre-signed URL Bedrock invocation error, falling back to SDK:', err?.message || err);
        // Don't set auth error backoff for pre-signed URL, try SDK next
      }
    }

    // 2. SDK Bedrock
    if (!this.bedrockClient) {
      this.initBedrockClient();
    }

    if (this.bedrockClient && Date.now() > this.bedrockAuthErrorUntil) {
      try {
        return await this.callBedrock<T>(systemInstruction, prompt, temperature, startTime);
      } catch (err: any) {
        console.warn('[LLMClient] Amazon Bedrock SDK invocation error, falling back to other providers:', err?.message || err);
      }
    }

    // 3. Try Gemini
    if (this.genAI) {
      for (const model of this.candidateGeminiModels) {
        try {
          const timeoutPromise = new Promise((_, reject) =>
            setTimeout(() => reject(new Error(`Timeout after 12000ms with model ${model}`)), 12000)
          );

          const generatePromise = this.genAI.models.generateContent({
            model,
            contents: prompt,
            config: {
              systemInstruction,
              temperature,
              responseMimeType: 'application/json',
            },
          });

          const response = (await Promise.race([generatePromise, timeoutPromise])) as any;

          const rawText = response.text || '{}';
          const latencyMs = Date.now() - startTime;
          const parsed = this.cleanAndParseJSON<T>(rawText);

          const promptTokens = Math.ceil((systemInstruction.length + prompt.length) / 4);
          const completionTokens = Math.ceil(rawText.length / 4);

          return {
            data: parsed,
            rawText,
            latencyMs,
            provider: 'gemini',
            model,
            estimatedTokens: {
              promptTokens,
              completionTokens,
              totalTokens: promptTokens + completionTokens,
            },
          };
        } catch (err: any) {
          console.warn(`[LLMClient] Attempt with Gemini model ${model} encountered error:`, err?.message || err);
          const msg = String(err?.message || '');
          if (
            msg.includes('API_KEY_INVALID') ||
            msg.includes('PERMISSION_DENIED') ||
            msg.includes('403') ||
            msg.includes('401') ||
            msg.includes('not found') ||
            msg.includes('404')
          ) {
            console.warn('[LLMClient] Non-retryable error with Gemini. Skipping remaining candidate models.');
            break;
          }
        }
      }
    }

    // 4. Try OpenAI if configured
    if (process.env.OPENAI_API_KEY) {
      try {
        return await this.callOpenAI<T>(systemInstruction, prompt, temperature, startTime);
      } catch (openAiErr: any) {
        console.warn('[LLMClient] OpenAI fallback encountered error:', openAiErr?.message);
      }
    }

    // 5. Resilient Heuristic Fallback (Guarantees zero crashes / continuous availability)
    console.info('[LLMClient] Applying resilient heuristic synthesis fallback for agent request.');
    const fallbackData = generateHeuristicFallback<T>(systemInstruction, prompt);
    const latencyMs = Date.now() - startTime;
    return {
      data: fallbackData,
      rawText: JSON.stringify(fallbackData),
      latencyMs: Math.max(latencyMs, 120),
      provider: 'fallback',
      model: 'heuristic-synthesis-engine',
      estimatedTokens: {
        promptTokens: Math.ceil(prompt.length / 4),
        completionTokens: 250,
        totalTokens: Math.ceil(prompt.length / 4) + 250,
      },
    };
  }

  private async callBedrockViaPreSignedUrl<T>(
    systemInstruction: string,
    prompt: string,
    temperature: number,
    startTime: number
  ): Promise<LLMResponse<T>> {
    if (!this.preSignedUrl) {
      throw new Error('Pre-signed URL not available');
    }

    // Amazon Nova format
    const body = JSON.stringify({
      inferenceConfig: {
        maxTokens: 6000,
        temperature,
      },
      messages: [
        {
          role: 'user',
          content: [
            {
              text: `${systemInstruction}\n\n${prompt}`,
            },
          ],
        },
      ],
    });

    const response = await fetch(this.preSignedUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`Bedrock pre-signed URL error (${response.status}): ${errorText}`);
    }

    const data = await response.json();
    // Nova response format: { output: { message: { content: [{ text: "..." }] } } }
    const rawText = data.output?.message?.content?.[0]?.text || '{}';
    const latencyMs = Date.now() - startTime;
    const parsed = this.cleanAndParseJSON<T>(rawText);

    const promptTokens = Math.ceil((systemInstruction.length + prompt.length) / 4);
    const completionTokens = Math.ceil(rawText.length / 4);

    console.log(`[LLMClient] Successfully executed via Bedrock pre-signed URL in ${latencyMs}ms`);

    return {
      data: parsed,
      rawText,
      latencyMs,
      provider: 'bedrock',
      model: 'pre-signed-url',
      estimatedTokens: {
        promptTokens,
        completionTokens,
        totalTokens: promptTokens + completionTokens,
      },
    };
  }

  private async callBedrock<T>(
    systemInstruction: string,
    prompt: string,
    temperature: number,
    startTime: number
  ): Promise<LLMResponse<T>> {
    if (!this.bedrockClient) {
      throw new Error('Amazon Bedrock client is not initialized');
    }

    const modelsToTry: string[] = [];
    if (process.env.BEDROCK_MODEL_ID) {
      modelsToTry.push(process.env.BEDROCK_MODEL_ID);
    }
    for (const m of this.candidateBedrockModels) {
      if (!modelsToTry.includes(m)) {
        modelsToTry.push(m);
      }
    }

    let lastError: any = null;

    for (const modelId of modelsToTry) {
      try {
        const strictJsonInstruction = `${systemInstruction}\n\nIMPORTANT: Output strictly valid JSON matching the requested schema. Do not enclose in conversational preamble or conversational closing.`;

        const command = new ConverseCommand({
          modelId,
          messages: [
            {
              role: 'user',
              content: [{ text: prompt }],
            },
          ],
          system: [{ text: strictJsonInstruction }],
          inferenceConfig: {
            temperature,
            maxTokens: 6000,
          },
        });

        const response = await this.bedrockClient.send(command);
        const rawText = response.output?.message?.content?.[0]?.text || '{}';
        const latencyMs = Date.now() - startTime;
        const parsed = this.cleanAndParseJSON<T>(rawText);

        const promptTokens =
          response.usage?.inputTokens || Math.ceil((systemInstruction.length + prompt.length) / 4);
        const completionTokens = response.usage?.outputTokens || Math.ceil(rawText.length / 4);

        console.log(`[LLMClient] Successfully executed via Amazon Bedrock SDK (${modelId}) in ${latencyMs}ms`);

        return {
          data: parsed,
          rawText,
          latencyMs,
          provider: 'bedrock',
          model: modelId,
          estimatedTokens: {
            promptTokens,
            completionTokens,
            totalTokens: promptTokens + completionTokens,
          },
        };
      } catch (modelErr: any) {
        lastError = modelErr;
        console.warn(`[LLMClient] Bedrock model ${modelId} failed:`, modelErr?.message || modelErr);

        const isAuthOrCredError =
          modelErr?.message?.includes('Could not load credentials') ||
          modelErr?.name === 'CredentialsProviderError' ||
          modelErr?.name === 'UnrecognizedClientException' ||
          modelErr?.name === 'AccessDeniedException' ||
          modelErr?.name === 'IncompleteSignature' ||
          modelErr?.message?.includes('security token included in the request is invalid');

        if (isAuthOrCredError) {
          this.bedrockAuthErrorUntil = Date.now() + 60000; // Back off for 60 seconds
          console.warn(`[LLMClient] Bedrock authentication error (${modelErr?.message || modelErr?.name}). Skipping candidate loop and backing off for 60s.`);
          break;
        }
      }
    }

    throw lastError || new Error('All Bedrock candidate models failed');
  }

  private async callOpenAI<T>(
    systemInstruction: string,
    prompt: string,
    temperature: number,
    startTime: number
  ): Promise<LLMResponse<T>> {
    const apiKey = process.env.OPENAI_API_KEY;
    if (!apiKey) {
      throw new Error('OPENAI_API_KEY is not available');
    }

    const response = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model: 'gpt-4o-mini',
        messages: [
          { role: 'system', content: systemInstruction },
          { role: 'user', content: prompt },
        ],
        response_format: { type: 'json_object' },
        temperature,
      }),
    });

    if (!response.ok) {
      const errBody = await response.text();
      throw new Error(`OpenAI API Error (${response.status}): ${errBody}`);
    }

    const json = await response.json();
    const rawText = json.choices?.[0]?.message?.content || '{}';
    const latencyMs = Date.now() - startTime;
    const parsed = this.cleanAndParseJSON<T>(rawText);

    return {
      data: parsed,
      rawText,
      latencyMs,
      provider: 'openai',
      model: 'gpt-4o-mini',
      estimatedTokens: {
        promptTokens: json.usage?.prompt_tokens || Math.ceil(prompt.length / 4),
        completionTokens: json.usage?.completion_tokens || Math.ceil(rawText.length / 4),
        totalTokens: json.usage?.total_tokens || 0,
      },
    };
  }

  private cleanAndParseJSON<T>(text: string): T {
    let clean = text.trim();
    if (clean.startsWith('```json')) {
      clean = clean.replace(/^```json\s*/, '').replace(/\s*```$/, '');
    } else if (clean.startsWith('```')) {
      clean = clean.replace(/^```\s*/, '').replace(/\s*```$/, '');
    }

    try {
      return JSON.parse(clean) as T;
    } catch (err) {
      const firstBrace = clean.indexOf('{');
      const lastBrace = clean.lastIndexOf('}');
      if (firstBrace !== -1 && lastBrace !== -1 && lastBrace > firstBrace) {
        const substring = clean.slice(firstBrace, lastBrace + 1);
        return JSON.parse(substring) as T;
      }
      throw new Error(`Failed to parse structured JSON from LLM response: ${err}`);
    }
  }
}

export const llmClient = new LLMClient();