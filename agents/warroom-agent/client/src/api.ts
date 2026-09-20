import {
  AskWarroomResponse,
  CompanyContext,
  CompetitiveSignal,
  ResponseSimulation,
  ScanResponse,
} from "./types";

const API_BASE_URL = "http://localhost:8000";

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
    this.name = "ApiError";
  }
}

export async function checkHealth(): Promise<{ status: string; service: string }> {
  const response = await fetch(`${API_BASE_URL}/api/health`);
  if (!response.ok) {
    throw new ApiError(`Health check failed: ${response.statusText}`, response.status);
  }
  return response.json();
}

export async function scanLandscape(): Promise<ScanResponse> {
  const response = await fetch(`${API_BASE_URL}/api/scan`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({}),
  });

  if (!response.ok) {
    let errorDetail = response.statusText;
    try {
      const data = await response.json();
      if (data.detail) {
        errorDetail = data.detail;
      }
    } catch {
      // ignore parsing error, use default statusText
    }
    throw new ApiError(errorDetail, response.status);
  }

  return response.json();
}

export async function simulateSignal(
  signal: CompetitiveSignal,
  company?: CompanyContext
): Promise<ResponseSimulation> {
  const response = await fetch(`${API_BASE_URL}/api/simulate`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      signal,
      company: company || null,
    }),
  });

  if (!response.ok) {
    let errorDetail = response.statusText;
    try {
      const data = await response.json();
      if (data.detail) {
        errorDetail = data.detail;
      }
    } catch {
      // ignore
    }
    throw new ApiError(errorDetail, response.status);
  }

  return response.json();
}

export async function askWarroom(
  question: string,
  signals: CompetitiveSignal[],
  company?: CompanyContext
): Promise<AskWarroomResponse> {
  const response = await fetch(`${API_BASE_URL}/api/ask`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      question,
      signals,
      company: company || null,
    }),
  });

  if (!response.ok) {
    let errorDetail = response.statusText;
    try {
      const data = await response.json();
      if (data.detail) {
        errorDetail = data.detail;
      }
    } catch {
      // ignore
    }
    throw new ApiError(errorDetail, response.status);
  }

  return response.json();
}
