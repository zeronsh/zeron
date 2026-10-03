import { Bash, InMemoryFs, MountableFs, defineCommand } from "just-bash/browser";
import { NativeWorkspaceFs } from "./native-fs.mjs";

// Deliberately small, foreground-only feasibility environment. No host file
// access, networking, package installation, Python, or arbitrary JS execution.
export const commands = [
  "cat", "echo", "printf", "pwd", "ls", "find", "grep", "rg", "sed", "awk",
  "head", "tail", "wc", "sort", "uniq", "cut", "tr", "diff", "jq",
  "mkdir", "touch", "cp", "mv", "rm", "basename", "dirname", "sleep",
];
const limits = {
  maxExecutionTimeMs: 60_000,
  maxLoopIterations: 10_000,
  maxCommandCount: 10_000,
  maxOutputSize: 256 * 1024,
  maxFileSystemBytes: 8 * 1024 * 1024,
  maxLiveBytes: 32 * 1024 * 1024,
};

function workspacePath(path) {
  if (typeof path !== "string" || !path.startsWith("/workspace/") || path.split("/").includes("..")) {
    throw new Error("Expected an absolute /workspace/ path without '..'");
  }
  return path;
}

export async function createRuntime(snapshot = [], nativeRequest) {
  const base = new InMemoryFs({}, { maxTotalBytes: limits.maxFileSystemBytes });
  const native = nativeRequest ? new NativeWorkspaceFs(nativeRequest) : null;
  const fs = native ? new MountableFs({ base, mounts: [{ mountPoint: "/workspace", filesystem: native }] }) : base;
  if (native) await native.refresh();
  await fs.mkdir("/workspace", { recursive: true });
  for (const entry of snapshot) {
    workspacePath(entry.path);
    if (entry.type === "directory") await fs.mkdir(entry.path, { recursive: true });
    else if (entry.type === "file") {
      await fs.mkdir(fs.resolvePath(entry.path, ".."), { recursive: true });
      await fs.writeFile(entry.path, entry.content, "base64");
    } else throw new Error("Unsupported workspace entry");
    await fs.chmod(entry.path, entry.mode);
  }
  return {
    async dispatch({ method, command, path, content }) {
      switch (method) {
        case "exec": {
          if (typeof command !== "string" || command.length > 64 * 1024) throw new Error("Invalid command");
          if (native) await native.refresh();
          // A fresh interpreter prevents cwd/environment state leaking across tool calls.
          const customCommands = native ? [
            ...[["render", "render"], ["import_image", "importImage"], ["git", "git"], ["pdf", "pdf"], ["serve", "serve"]].map(([name, method]) => defineCommand(name, async (args, context) => {
              try {
                const output = await native.call(method, "/", { args, cwd: context.cwd });
                return typeof output === "object" ? output : { stdout: output + "\n", stderr: "", exitCode: 0 };
              } catch (error) { return { stdout: "", stderr: String(error) + "\n", exitCode: 1 }; }
            })),
          ] : [];
          const interpreter = new Bash({ fs, cwd: "/workspace", commands, customCommands, executionLimits: limits });
          const { stdout, stderr, exitCode } = await interpreter.exec(command, { cwd: "/workspace" });
          return { stdout, stderr, exitCode };
        }
        case "read": return await fs.readFile(workspacePath(path));
        case "write": await fs.writeFile(workspacePath(path), content); return null;
        case "snapshot": {
          const entries = [];
          for (const path of fs.getAllPaths().filter(p => p.startsWith("/workspace/")).sort()) {
            const stat = await fs.lstat(path);
            if (stat.isSymbolicLink) throw new Error("Symlinks are not supported by the mobile checkpoint format");
            entries.push(stat.isDirectory
              ? { path, type: "directory", mode: stat.mode }
              : { path, type: "file", mode: stat.mode, content: await fs.readFile(path, "base64") });
          }
          return entries;
        }
        default: throw new Error(`Unknown mobile tool: ${method}`);
      }
    },
  };
}
