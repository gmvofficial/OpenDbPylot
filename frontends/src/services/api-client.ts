// API client for the opendbpylot backend.
//
// An SSE transport exposed as an async generator
// of {rich, simple} chunks, with auth header passthrough and id generation.
// (WebSocket / polling transports can be added later for full parity.)

export interface ChatRequest {
  message: string;
  conversation_id?: string;
  request_id?: string;
  metadata?: Record<string, any>;
}

export interface RichComponent {
  id: string;
  type: string;
  lifecycle?: "create" | "update" | "replace" | "remove";
  data: Record<string, any>;
  children?: string[];
  visible?: boolean;
  interactive?: boolean;
  timestamp?: string;
}

export interface ChatStreamChunk {
  rich?: RichComponent;
  simple?: Record<string, any>;
  conversation_id?: string;
  request_id?: string;
  timestamp?: number;
}

export interface ApiClientConfig {
  baseUrl?: string;
  sseEndpoint?: string;
  pollEndpoint?: string;
  wsEndpoint?: string;
  starterEndpoint?: string;
  customHeaders?: Record<string, string>;
}

export type Transport = "sse" | "poll" | "ws";

export class OpenDbPylotApiClient {
  baseUrl: string;
  sseEndpoint: string;
  pollEndpoint: string;
  wsEndpoint: string;
  starterEndpoint: string;
  customHeaders: Record<string, string>;

  constructor(config: ApiClientConfig = {}) {
    this.baseUrl = config.baseUrl || "";
    this.sseEndpoint = config.sseEndpoint || "/api/opendbpylot/v2/chat_sse";
    this.pollEndpoint = config.pollEndpoint || "/api/opendbpylot/v2/chat_poll";
    this.wsEndpoint = config.wsEndpoint || "/api/opendbpylot/v2/chat_websocket";
    this.starterEndpoint = config.starterEndpoint || "/api/opendbpylot/v2/starter";
    this.customHeaders = config.customHeaders || {};
  }

  private resolve(path: string): string {
    return path.startsWith("http") ? path : `${this.baseUrl}${path}`;
  }

  /** Pick a transport and return a uniform async generator of chunks. */
  stream(transport: Transport, request: ChatRequest): AsyncGenerator<ChatStreamChunk, void, unknown> {
    if (transport === "poll") return this.pollChat(request);
    if (transport === "ws") return this.wsChat(request);
    return this.streamChat(request);
  }

  /** Fetch starter prompt suggestions (returns [] on failure). */
  async getStarter(): Promise<string[]> {
    try {
      const res = await fetch(this.resolve(this.starterEndpoint), { headers: this.customHeaders });
      if (!res.ok) return [];
      const data = await res.json();
      return data.suggestions || [];
    } catch {
      return [];
    }
  }

  /** Set auth headers (e.g. Authorization / cookies-derived tokens). */
  setCustomHeaders(headers: Record<string, string>) {
    this.customHeaders = headers;
  }

  generateId(): string {
    return `${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
  }

  /** Stream a chat response over SSE as an async generator of chunks. */
  async *streamChat(request: ChatRequest): AsyncGenerator<ChatStreamChunk, void, unknown> {
    const url = this.resolve(this.sseEndpoint);

    const response = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "text/event-stream",
        ...this.customHeaders,
      },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }
    const reader = response.body?.getReader();
    if (!reader) throw new Error("No response body");

    const decoder = new TextDecoder();
    let buffer = "";

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";
        for (const line of lines) {
          if (!line.startsWith("data: ")) continue;
          const data = line.slice(6).trim();
          if (data === "[DONE]") return;
          try {
            yield JSON.parse(data) as ChatStreamChunk;
          } catch (e) {
            console.warn("Failed to parse SSE chunk:", data, e);
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  /** Polling transport: one request returns all chunks; we yield them. */
  async *pollChat(request: ChatRequest): AsyncGenerator<ChatStreamChunk, void, unknown> {
    const res = await fetch(this.resolve(this.pollEndpoint), {
      method: "POST",
      headers: { "Content-Type": "application/json", ...this.customHeaders },
      body: JSON.stringify(request),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}: ${res.statusText}`);
    const data = await res.json();
    for (const chunk of data.chunks || []) {
      yield chunk as ChatStreamChunk;
    }
  }

  /** WebSocket transport: yields chunks until a `completion` message. */
  async *wsChat(request: ChatRequest): AsyncGenerator<ChatStreamChunk, void, unknown> {
    const ws = new WebSocket(this.wsUrl());
    await new Promise<void>((resolve, reject) => {
      ws.onopen = () => resolve();
      ws.onerror = () => reject(new Error("WebSocket connection failed"));
    });

    const queue: ChatStreamChunk[] = [];
    let done = false;
    let wake: (() => void) | null = null;
    const bump = () => {
      if (wake) {
        wake();
        wake = null;
      }
    };

    ws.onmessage = (e) => {
      try {
        const chunk = JSON.parse(e.data) as ChatStreamChunk;
        if ((chunk.rich as any)?.type === "completion") done = true;
        else queue.push(chunk);
      } catch {
        /* ignore */
      }
      bump();
    };
    ws.onclose = () => {
      done = true;
      bump();
    };

    ws.send(JSON.stringify(request));

    try {
      while (true) {
        if (queue.length) {
          yield queue.shift()!;
          continue;
        }
        if (done) break;
        await new Promise<void>((r) => (wake = r));
      }
    } finally {
      ws.close();
    }
  }

  private wsUrl(): string {
    if (this.wsEndpoint.startsWith("ws")) return this.wsEndpoint;
    const base = this.baseUrl || `${location.protocol}//${location.host}`;
    const u = new URL(this.wsEndpoint, base);
    u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
    return u.toString();
  }
}
