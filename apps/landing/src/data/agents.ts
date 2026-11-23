// The agents Zeron runs. `mark` names a file in src/assets/agents, drawn as
// a mask so it takes the colour of the text around it.
export const AGENTS = [
  { mark: "claude", name: "Claude Code" },
  { mark: "openai", name: "Codex" },
  { mark: "cursor", name: "Cursor" },
  { mark: "opencode", name: "OpenCode" },
  { mark: "antigravity", name: "Antigravity" },
  { mark: "devin", name: "Devin" },
  { mark: "grok", name: "Grok" },
  { mark: "hermes", name: "Hermes" },
  { mark: "pi", name: "Pi" },
] as const;

export type AgentMark = (typeof AGENTS)[number]["mark"];
