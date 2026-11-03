export interface ProviderCfg {
  type: string;
  model: string;
  base_url: string | null;
  api_key: string | null;
  max_tokens: number | null;
  temperature: number | null;
  top_p: number | null;
  stream: boolean;
  active?: string | null;
}

export interface McpCfg {
  name: string;
  type: string;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
  headers?: Record<string, string>;
  timeout?: number;
}

export interface RuleCfg {
  tool: string;
  pattern?: string;
  permission: string;
}

export interface Cfg {
  provider: ProviderCfg;
  profiles?: unknown;
  mcp: McpCfg[];
  keys: Record<string, string>;
  ui: { theme: string };
  agent?: unknown;
  permissions?: { [k: string]: unknown; rules?: RuleCfg[] };
  lsp?: unknown;
  formatters?: unknown;
}

export interface SessionMeta {
  id: string;
  title: string;
  created: number;
  updated: number;
  count: number;
  cost?: number;
}

export interface TranscriptItem {
  k: "user" | "bot" | "tool" | "toolout";
  s: string;
}

export interface DiffRow {
  tag: 0 | 1 | 2;
  text: string;
}

export interface ChangeEntry {
  adds: number;
  dels: number;
  rows: DiffRow[] | null;
  label?: string;
}

export interface OpenSession {
  id: string;
  title: string;
  created: number;
  updated: number;
  transcript: TranscriptItem[];
  changes: { path: string; adds: number; dels: number }[];
  queue?: string[];
  tokens_in?: number;
  tokens_out?: number;
  cost?: number;
}

export interface InitState {
  cfg: Cfg;
  keys: Record<string, string>;
  cwd: string;
  config_path?: string;
  has_provider: boolean;
  sessions: SessionMeta[];
  sid: string;
  title: string;
  theme: string;
  plan?: boolean;
  todos?: string;
}

export type Ev =
  | { t: "chunk"; s: string }
  | { t: "reasoning"; s: string }
  | { t: "note"; s: string }
  | { t: "tool"; name: string; detail: string; diff?: DiffRow[]; paths?: string[] }
  | { t: "confirm"; name: string; detail: string; diff?: DiffRow[] }
  | { t: "ask"; args: string }
  | { t: "todo"; s: string }
  | { t: "bgout"; id: string; s: string }
  | { t: "plan"; on: boolean }
  | {
      t: "usage";
      input: number;
      output: number;
      cached?: number;
      cost?: number;
      ctx_used?: number;
      ctx_limit?: number;
    }
  | { t: "attachments"; list: string[] }
  | { t: "queue"; list: string[] }
  | { t: "quit_ask" }
  | { t: "sessions"; list: SessionMeta[]; sid?: string }
  | { t: "cleared" }
  | { t: "done"; text: string }
  | { t: "failed"; s: string }
  | { t: "idle" }
  | { t: "queued" }
  | { t: "model"; name: string; kind?: string }
  | { t: "theme"; name: string }
  | { t: "crew"; id: string; author: string }
  | { t: "crewchunk"; id: string; author: string; s: string }
  | { t: "bell" };

export interface DirEntry {
  name: string;
  dir: boolean;
  size: number;
}

export interface DirList {
  path: string;
  cwd: string;
  is_cwd: boolean;
  parent: string | null;
  entries: DirEntry[];
}

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  header?: string;
  question?: string;
  multiple?: boolean;
  options?: AskOption[];
  schema?: {
    properties?: Record<
      string,
      {
        type?: string;
        description?: string;
        default?: unknown;
        enum?: unknown[];
        enumNames?: string[];
        items?: { enum?: unknown[]; enumNames?: string[] };
        minimum?: number;
        maximum?: number;
        maxLength?: number;
        format?: string;
      }
    >;
    required?: string[];
  };
}

export interface SandboxSpec {
  id?: string;
  name: string;
  kind: string;
  login?: string;
  iso_path?: string | null;
  disk_gib: number;
  ram_mib: number;
  cpus: number;
  root: boolean;
  ssh_port: number;
  forwards?: { host_port: number; guest_host: string; guest_port: number }[];
}

export interface SandboxInfo {
  spec: SandboxSpec;
  state: string;
  error?: string | null;
  download?: { downloaded: number; total: number } | null;
  ssh?: {
    state: string;
    error?: string;
    user?: string;
    port?: number;
    elapsed_secs?: number;
    agent?: { state: string; version?: string; error?: string; log?: string[] };
  } | null;
}

export interface SandboxDetect {
  system_path?: string;
  img_path?: string;
  system_version?: string;
  accel?: string;
}

export interface HostInfo {
  id: string;
  name: string;
  label: string;
  state: string;
  error?: string | null;
  agent?: boolean | null;
}

export interface SkillInfo {
  name: string;
  description?: string;
  path: string;
  enabled: boolean;
}

export interface CrewMember {
  name: string;
  role: string;
  model?: string;
  tools?: boolean;
  review?: boolean;
  bg?: boolean;
  limit?: number;
  used?: number;
}

export interface CrewState {
  id?: string;
  goal: string;
  status: string;
  running: boolean;
  bg_running?: number;
  budget?: number;
  spent?: number;
  members: CrewMember[];
  messages: { author: string; kind: string; content: string }[];
  memory?: { author: string; content: string }[];
  graph?: { from: string; to: string; n: number }[];
  none?: boolean;
}

export interface CrewSaved {
  id: string;
  goal: string;
  status: string;
  members: number;
  messages: number;
  updated: number;
  mentions?: { from: string; to: string; n: number }[];
}

export interface CrewPreset {
  name: string;
  goal: string;
  members: number;
  budget?: number;
}

export interface McpResource {
  server: string;
  uri: string;
  name?: string;
  description?: string;
  mime?: string;
}

export interface McpPrompt {
  server: string;
  name: string;
  description?: string;
  arguments?: { name: string; description?: string; required?: boolean }[];
}

export interface McpTemplate {
  server: string;
  uri_template: string;
  name?: string;
  description?: string;
  mime?: string;
}
