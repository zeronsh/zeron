/**
 * The models Cloud offers, per agent: what the phone (or any client without an
 * engine of its own) shows in a Cloud chat's model picker. Kept by hand, so a
 * new model ships with an edge deploy, not an app release. Match it to the
 * CLIs Cloud machines install (last checked: Claude Code 2.1.307, Codex
 * 0.163.0). Each agent's first model is its default.
 */

export interface ModelOption {
  readonly id: string;
  readonly label: string;
  readonly choices: readonly { readonly id: string; readonly label: string }[];
  readonly defaultChoice: string;
}

export interface CloudModel {
  readonly id: string;
  readonly label: string;
  readonly description: string;
  readonly reasoningLevels: readonly string[];
  readonly options: readonly ModelOption[];
}

export type CloudHarness = "claude-code" | "codex";

const CLAUDE_FULL = ["low", "medium", "high", "xhigh", "max", "ultracode", "ultrathink"];
const CLAUDE_XHIGH = ["low", "medium", "high", "xhigh", "max", "ultrathink"];
const CODEX_ULTRA = ["low", "medium", "high", "xhigh", "max", "ultra"];
const CODEX_MAX = ["low", "medium", "high", "xhigh", "max"];

const toggle = (id: string, label: string): ModelOption => ({
  id,
  label,
  choices: [
    { id: "off", label: "Off" },
    { id: "on", label: "On" },
  ],
  defaultChoice: "off",
});

const contextWindow: ModelOption = {
  id: "contextWindow",
  label: "Context Window",
  choices: [
    { id: "200k", label: "200K" },
    { id: "1m", label: "1M" },
  ],
  defaultChoice: "200k",
};

const serviceTier: ModelOption = {
  id: "serviceTier",
  label: "Service Tier",
  choices: [
    { id: "default", label: "Standard" },
    { id: "fast", label: "Fast" },
  ],
  defaultChoice: "default",
};

const model = (
  id: string,
  label: string,
  description: string,
  reasoningLevels: readonly string[],
  options: readonly ModelOption[] = []
): CloudModel => ({ id, label, description, reasoningLevels, options });

export const CLOUD_MODELS: Readonly<Record<CloudHarness, readonly CloudModel[]>> = {
  "claude-code": [
    model("claude-opus-5-5", "Opus 5.5", "Best for everyday, complex tasks", CLAUDE_FULL, [contextWindow, toggle("fastMode", "Fast Mode")]),
    model("claude-fable-5-1", "Fable 5.1", "Most intelligent model for building agents", CLAUDE_FULL, [contextWindow]),
    model("claude-sonnet-5-5", "Sonnet 5.5", "Most efficient for simpler tasks", CLAUDE_FULL),
    model("claude-haiku-5-5", "Haiku 5.5", "Fastest for quick answers", CLAUDE_FULL),
    model("claude-fable-5", "Fable 5", "Previous generation Fable", CLAUDE_FULL, [contextWindow]),
    model("claude-opus-5", "Opus 5", "Previous generation Opus", CLAUDE_FULL),
    model("claude-opus-4-8", "Opus 4.8", "Older generation Opus", CLAUDE_FULL, [toggle("fastMode", "Fast Mode")]),
    model("claude-sonnet-5", "Sonnet 5", "Previous generation Sonnet", CLAUDE_XHIGH, [contextWindow]),
    model("claude-haiku-4-5", "Haiku 4.5", "Previous generation Haiku", [], [toggle("thinking", "Thinking")]),
  ],
  codex: [
    model("gpt-6.1-sol", "GPT-6.1-Sol", "Latest workhorse model for coding and everyday work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-astra", "GPT-6-Astra", "Frontier intelligence for the most demanding work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-sol", "GPT-6-Sol", "Previous generation workhorse model.", CODEX_ULTRA, [serviceTier]),
    model("gpt-6-luna", "GPT-6-Luna", "Fast and affordable model for easier tasks.", CODEX_MAX, [serviceTier]),
    model("gpt-5.6-sol", "GPT-5.6-Sol", "Older generation workhorse model.", CODEX_ULTRA, [serviceTier]),
    model("gpt-5.6-terra", "GPT-5.6-Terra", "Older balanced model for straightforward work.", CODEX_ULTRA, [serviceTier]),
    model("gpt-5.6-luna", "GPT-5.6-Luna", "Older fast and efficient model.", CODEX_MAX, [serviceTier]),
    model("gpt-daybreak-blue-latest", "Daybreak Blue", "Latest frontier agentic coding model for broad defensive cybersecurity work.", CODEX_ULTRA),
  ],
};
