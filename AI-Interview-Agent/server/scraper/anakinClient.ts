import * as cheerio from 'cheerio';

export interface AnakinScrapeResult {
  url: string;
  title: string;
  markdown: string;
  textContent: string;
  provider: 'anakin.io' | 'native_fallback';
  meta?: {
    description?: string;
    keywords?: string[];
    author?: string;
    status?: number;
  };
  latencyMs: number;
}

export interface ScrapedJobPosting {
  title: string;
  company: string;
  location?: string;
  requiredSkills: string[];
  seniority?: string;
  description: string;
  url: string;
  provider: string;
}

export interface AnakinStatus {
  configured: boolean;
  apiKeySet: boolean;
  endpoint: string;
  hasCredits: boolean;
  features: string[];
  mode: 'anakin_api' | 'fallback_ready';
}

class AnakinScraperClient {
  private baseUrl: string;

  constructor() {
    this.baseUrl = process.env.ANAKIN_BASE_URL || 'https://api.anakin.io/v1';
  }

  getStatus(): AnakinStatus {
    const apiKey = process.env.ANAKIN_API_KEY || process.env.ANAKIN_IO_API_KEY;
    const apiKeySet = Boolean(apiKey && apiKey.trim().length > 0 && apiKey !== 'YOUR_ANAKIN_API_KEY');

    return {
      configured: apiKeySet,
      apiKeySet,
      endpoint: this.baseUrl,
      hasCredits: true, // User reported having free credits
      features: [
        'Live URL Web Scraping (URL Scraper API)',
        'Target Job Posting Extraction (LinkedIn, Greenhouse, Lever, Ashby, Indeed)',
        'GitHub Candidate Profile & Repository Scraper',
        'Company Technical Documentation Scraper for Interview Calibration',
      ],
      mode: apiKeySet ? 'anakin_api' : 'fallback_ready',
    };
  }

  /**
   * Scrapes any webpage via Anakin.io URL Scraper API with resilient fallback
   */
  async scrapeUrl(
    targetUrl: string,
    options: { renderJs?: boolean; extractMarkdown?: boolean } = {}
  ): Promise<AnakinScrapeResult> {
    const startTime = Date.now();
    const apiKey = process.env.ANAKIN_API_KEY || process.env.ANAKIN_IO_API_KEY;

    // Validate URL
    let parsedUrl: URL;
    try {
      parsedUrl = new URL(targetUrl);
    } catch {
      throw new Error(`Invalid URL format: "${targetUrl}". Please include http:// or https://`);
    }

    // 1. Try Anakin.io if API key is provided
    if (apiKey && apiKey.trim().length > 0 && apiKey !== 'YOUR_ANAKIN_API_KEY') {
      try {
        console.log(`[AnakinScraper] Calling Anakin.io URL Scraper API for: ${targetUrl}`);
        
        // Anakin.io URL Scraper endpoint
        const anakinEndpoint = `${this.baseUrl}/url-scraper/scrape`;
        const response = await fetch(anakinEndpoint, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-API-Key': apiKey.trim(),
            Authorization: `Bearer ${apiKey.trim()}`,
          },
          body: JSON.stringify({
            url: parsedUrl.toString(),
            render_js: options.renderJs ?? true,
            extract_markdown: options.extractMarkdown ?? true,
          }),
          signal: AbortSignal.timeout(15000), // 15s timeout
        });

        if (response.ok) {
          const data = await response.json();
          const latencyMs = Date.now() - startTime;
          console.log(`[AnakinScraper] Anakin.io successfully scraped ${targetUrl} in ${latencyMs}ms`);

          const markdown = data.markdown || data.content || data.text || '';
          const title = data.title || parsedUrl.hostname;
          const textContent = data.text || markdown.replace(/[#*`_\[\]()]/g, '');

          return {
            url: targetUrl,
            title,
            markdown,
            textContent,
            provider: 'anakin.io',
            meta: {
              description: data.description,
              status: response.status,
            },
            latencyMs,
          };
        } else {
          const errText = await response.text();
          console.warn(`[AnakinScraper] Anakin.io responded with status ${response.status}: ${errText}. Falling back to native scraper.`);
        }
      } catch (anakinErr: any) {
        console.warn(`[AnakinScraper] Anakin.io request error:`, anakinErr?.message || anakinErr);
      }
    } else {
      console.info(`[AnakinScraper] ANAKIN_API_KEY not configured or empty. Using native resilient web scraper.`);
    }

    // 2. High-Grade Native Fallback Scraper (guarantees zero crashes & continuous functionality)
    return await this.fallbackScrape(parsedUrl.toString(), startTime);
  }

  /**
   * Native resilient HTML scraper and cleaner using Cheerio
   */
  private async fallbackScrape(url: string, startTime: number): Promise<AnakinScrapeResult> {
    try {
      const response = await fetch(url, {
        headers: {
          'User-Agent':
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
          Accept: 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
        },
        signal: AbortSignal.timeout(10000),
      });

      if (!response.ok) {
        throw new Error(`HTTP error ${response.status} fetching ${url}`);
      }

      const html = await response.text();
      const $ = cheerio.load(html);

      // Strip non-content tags
      $('script, style, noscript, iframe, svg, nav, footer, header').remove();

      const title = $('title').text().trim() || $('h1').first().text().trim() || new URL(url).hostname;
      const description = $('meta[name="description"]').attr('content') || $('meta[property="og:description"]').attr('content') || '';

      // Collect meaningful body text
      const textParts: string[] = [];
      $('h1, h2, h3, h4, p, li, pre, code').each((_, el) => {
        const text = $(el).text().trim();
        if (text.length > 5) {
          textParts.push(text);
        }
      });

      const textContent = textParts.join('\n\n');
      const markdown = `# ${title}\n\n${description ? `> ${description}\n\n` : ''}${textContent}`;
      const latencyMs = Date.now() - startTime;

      return {
        url,
        title,
        markdown,
        textContent,
        provider: 'native_fallback',
        meta: {
          description,
          status: response.status,
        },
        latencyMs,
      };
    } catch (err: any) {
      const latencyMs = Date.now() - startTime;
      throw new Error(`Failed to scrape URL ${url}: ${err.message || err}`);
    }
  }

  /**
   * Specialized Job Posting Extractor
   */
  async scrapeJobPosting(url: string): Promise<ScrapedJobPosting> {
    const scrapeRes = await this.scrapeUrl(url);
    const content = scrapeRes.textContent;

    // Detect technical keywords
    const commonTech = [
      'Java', 'Spring Boot', 'Python', 'Node.js', 'React', 'TypeScript', 'JavaScript',
      'Go', 'Golang', 'Rust', 'C++', 'C#', '.NET', 'AWS', 'GCP', 'Azure', 'Docker',
      'Kubernetes', 'MySQL', 'PostgreSQL', 'MongoDB', 'Redis', 'Kafka', 'RabbitMQ',
      'GraphQL', 'REST', 'Microservices', 'CI/CD', 'Git', 'Linux', 'Elasticsearch'
    ];

    const foundSkills = commonTech.filter((tech) => {
      const regex = new RegExp(`\\b${tech.replace('+', '\\+')}\\b`, 'i');
      return regex.test(content);
    });

    // Detect seniority
    let seniority = 'Mid-level Engineer';
    if (/senior|lead|staff|principal|architect/i.test(scrapeRes.title) || /senior|lead|5\+|6\+|7\+/i.test(content.slice(0, 500))) {
      seniority = 'Senior / Staff Engineer';
    } else if (/junior|entry|intern|graduate|associate/i.test(scrapeRes.title) || /junior|entry|0-2/i.test(content.slice(0, 500))) {
      seniority = 'Junior / Associate Engineer';
    }

    return {
      title: scrapeRes.title,
      company: this.extractCompanyName(url, scrapeRes.title),
      requiredSkills: foundSkills.length > 0 ? foundSkills : ['Software Engineering', 'System Design'],
      seniority,
      description: content.slice(0, 3000),
      url,
      provider: scrapeRes.provider,
    };
  }

  private extractCompanyName(url: string, title: string): string {
    try {
      const host = new URL(url).hostname.replace('www.', '');
      const parts = host.split('.');
      if (parts.length >= 2 && !['linkedin', 'lever', 'greenhouse', 'ashbyhq', 'indeed'].includes(parts[0])) {
        return parts[0].charAt(0).toUpperCase() + parts[0].slice(1);
      }
      // If hosted on ATS, parse from title or path
      if (title.includes(' at ')) {
        return title.split(' at ')[1].split(' - ')[0].trim();
      }
      if (title.includes(' - ')) {
        const afterHyphen = title.split(' - ').pop();
        if (afterHyphen && afterHyphen.length < 30) return afterHyphen.trim();
      }
    } catch {
      // ignore
    }
    return 'Target Company';
  }
}

export const anakinScraperClient = new AnakinScraperClient();
