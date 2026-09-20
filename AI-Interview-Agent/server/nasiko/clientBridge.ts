import { AgentName } from './contracts';

export interface NasikoBridgeStatus {
  mode: 'docker_nasiko' | 'embedded_fallback';
  targetUrl: string;
  isDockerReachable: boolean;
  isOtelReachable: boolean;
  lastCheckedAt: string;
  registeredAgents: {
    agentName: AgentName;
    displayName: string;
    registered: boolean;
  }[];
  dockerInfo: {
    repo: string;
    command: string;
    defaultHost: string;
    statusSummary: string;
  };
}

class NasikoClientBridge {
  private targetUrl: string;
  private otelUrl: string;
  private isDockerReachable = false;
  private isOtelReachable = false;
  private lastCheckedAt = new Date().toISOString();
  private checking = false;

  constructor() {
    // Default to http://localhost:8080 which is Nasiko's default exposed Docker port
    this.targetUrl = (process.env.NASIKO_API_URL || 'http://localhost:8080').replace(/\/+$/, '');
    this.otelUrl = (process.env.OTEL_EXPORTER_OTLP_ENDPOINT || 'http://localhost:4318').replace(/\/+$/, '');
    
    // Initial probe
    this.probeDockerInstance().catch(() => {});
  }

  getTargetUrl(): string {
    return this.targetUrl;
  }

  async probeDockerInstance(): Promise<boolean> {
    if (this.checking) return this.isDockerReachable;
    this.checking = true;
    this.lastCheckedAt = new Date().toISOString();

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 1500);

      // Probe Nasiko server
      try {
        const res = await fetch(`${this.targetUrl}/health`, {
          method: 'GET',
          signal: controller.signal,
        }).catch(() => null);

        if (res && (res.ok || res.status < 500)) {
          this.isDockerReachable = true;
        } else {
          // Probe root
          const rootRes = await fetch(`${this.targetUrl}/`, {
            method: 'GET',
            signal: controller.signal,
          }).catch(() => null);
          this.isDockerReachable = Boolean(rootRes && rootRes.status < 500);
        }
      } catch {
        this.isDockerReachable = false;
      }

      // Probe OTel Collector
      try {
        const otelRes = await fetch(`${this.otelUrl}/v1/traces`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({}),
          signal: controller.signal,
        }).catch(() => null);
        this.isOtelReachable = Boolean(otelRes && otelRes.status !== 0);
      } catch {
        this.isOtelReachable = false;
      }

      clearTimeout(timeoutId);
    } catch {
      this.isDockerReachable = false;
    } finally {
      this.checking = false;
    }

    return this.isDockerReachable;
  }

  async syncSpanToNasikoDocker(span: any): Promise<void> {
    if (!this.isDockerReachable && !this.isOtelReachable) {
      return;
    }

    // Attempt to forward trace to Nasiko server or OTel endpoint
    try {
      if (this.isDockerReachable) {
        await fetch(`${this.targetUrl}/api/v1/traces`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(span),
        }).catch(() => {});
      }
    } catch {
      // Non-blocking background sync
    }
  }

  getStatus(): NasikoBridgeStatus {
    const agentsList: { agentName: AgentName; displayName: string; registered: boolean }[] = [
      { agentName: 'resume_agent', displayName: 'Resume Reading Agent', registered: true },
      { agentName: 'interview_agent', displayName: 'Interview Agent', registered: true },
      { agentName: 'evaluation_agent', displayName: 'Evaluation Agent', registered: true },
      { agentName: 'report_agent', displayName: 'Interview Report Agent', registered: true },
      { agentName: 'preparation_agent', displayName: 'Personalized Preparation Plan Agent', registered: true },
    ];

    return {
      mode: this.isDockerReachable ? 'docker_nasiko' : 'embedded_fallback',
      targetUrl: this.targetUrl,
      isDockerReachable: this.isDockerReachable,
      isOtelReachable: this.isOtelReachable,
      lastCheckedAt: this.lastCheckedAt,
      registeredAgents: agentsList,
      dockerInfo: {
        repo: 'https://github.com/Nasiko-Labs/nasiko.git',
        command: 'git clone https://github.com/Nasiko-Labs/nasiko.git && cd nasiko && docker compose up -d',
        defaultHost: 'http://localhost:8080',
        statusSummary: this.isDockerReachable
          ? `Connected to Docker Nasiko Control Plane at ${this.targetUrl}`
          : `Running Embedded Nasiko Orchestration Layer. If Docker is running at ${this.targetUrl}, it auto-connects.`,
      },
    };
  }
}

export const nasikoClientBridge = new NasikoClientBridge();
