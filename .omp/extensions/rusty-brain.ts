// rusty-brain-owned-asset:v1 sha256=c1c310654ef7023ffe5c8a0070d97db6919071ef7d34017731685e2a5fd508ac
// Standalone native OMP bridge. Runtime dependencies are Bun and Node builtins only.
import type { ExtensionAPI, ExtensionContext } from "@oh-my-pi/pi-coding-agent";
import { randomUUID } from "node:crypto";
import { appendFile, mkdir, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

const INSTALLED_HOOKS_BINARY = "/Users/bluby/.cargo/bin/rusty-brain-hooks";
const HOOK_TIMEOUT_MS = 6500;
const MAX_INPUT_BYTES = 1024 * 1024;
const MAX_OUTPUT_BYTES = 256 * 1024;
const MAX_TRANSCRIPT_BYTES = 256 * 1024;
const MAX_PLACEBO_SOURCE_BYTES = 4 * 1024 * 1024;

type JsonObject = Record<string, unknown>;
type SessionState = {
  id: string;
  tail: Promise<void>;
  placeboInvocation: number;
  placeboReceipts?: Promise<string[] | undefined>;
  closed: boolean;
};

function objectValue(value: unknown): JsonObject | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as JsonObject
    : undefined;
}

function prefix(text: string, length: number): string {
  // Never cut a UTF-16 surrogate pair at a size boundary.
  const end = Math.min(length, text.length);
  const last = text.charCodeAt(end - 1);
  return text.slice(0, last >= 0xd800 && last <= 0xdbff ? end - 1 : end);
}

function textContent(value: unknown, limit = MAX_TRANSCRIPT_BYTES): string {
  if (typeof value === "string") return prefix(value, limit);
  if (!Array.isArray(value)) return "";
  const parts: string[] = [];
  let remaining = limit;
  for (const valueBlock of value) {
    const block = objectValue(valueBlock);
    if (block?.type !== "text" || typeof block.text !== "string" || !block.text) continue;
    if (parts.length) remaining -= 1;
    if (remaining <= 0) break;
    const text = prefix(block.text, remaining);
    parts.push(text);
    remaining -= text.length;
    if (remaining <= 0) break;
  }
  return parts.join("\n");
}

function transcript(context: ExtensionContext): string {
  try {
    const branch = context.sessionManager.getBranch();
    const lines: string[] = [];
    let remaining = MAX_TRANSCRIPT_BYTES;
    // Keep recent conversation, emitting complete JSONL records in branch order.
    for (let index = branch.length - 1; index >= 0; index -= 1) {
      const entry = objectValue(branch[index]);
      if (entry?.type !== "message") continue;
      const message = objectValue(entry.message);
      const role = message?.role;
      if (role !== "user" && role !== "assistant") continue;
      const content = textContent(message?.content);
      if (!content) continue;
      const lineFor = (text: string) => `${JSON.stringify({ message: { role, content: text } })}\n`;
      let line = lineFor(content);
      if (Buffer.byteLength(line) > remaining) {
        let low = 0;
        let high = content.length;
        while (low < high) {
          const middle = Math.ceil((low + high) / 2);
          if (Buffer.byteLength(lineFor(prefix(content, middle))) <= remaining) low = middle;
          else high = middle - 1;
        }
        const trimmed = prefix(content, low);
        if (!trimmed) break;
        line = lineFor(trimmed);
      }
      lines.push(line);
      remaining -= Buffer.byteLength(line);
    }
    return lines.reverse().join("");
  } catch {
    return "";
  }
}

async function readBounded(reader: ReadableStreamDefaultReader<Uint8Array>, limit: number): Promise<string> {
  const decoder = new TextDecoder();
  const parts: string[] = [];
  let bytes = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > limit) throw new Error("OMP bridge byte limit exceeded");
    parts.push(decoder.decode(value, { stream: true }));
  }
  parts.push(decoder.decode());
  return parts.join("");
}

export default function rustyBrainOmpExtension(pi: ExtensionAPI): void {
  // Capture configuration per factory, not per imported module or process-global session.
  const hooksBinary = process.env.RB_OMP_HOOKS_BIN ?? INSTALLED_HOOKS_BINARY;
  const receiptDirectory = process.env.RB_SCORECARD_INJECTION_DIR;
  const placeboSource = process.env.RB_SCORECARD_PLACEBO_SOURCE;
  const placebo = placeboSource !== undefined;
  const forbidden = (process.env.RB_SCORECARD_FORBIDDEN ?? "").split("\n").filter(Boolean);
  const sessions = new Map<string, SessionState>();
  const anonymousIds = new WeakMap<object, string>();
  let receipts: Promise<void> = Promise.resolve();

  function stateFor(context: ExtensionContext, resetAnonymous = false): SessionState {
    let id: unknown;
    try {
      id = context.sessionManager.getSessionId();
    } catch {
      // Some hosts cannot expose an ID yet. Never merge them into an "unknown" session.
    }
    if (typeof id !== "string" || !id.trim()) {
      id = resetAnonymous ? undefined : anonymousIds.get(context.sessionManager);
      if (!id) {
        id = randomUUID();
        anonymousIds.set(context.sessionManager, id as string);
      }
    }
    const key = id as string;
    let state = sessions.get(key);
    if (!state) {
      state = { id: key, tail: Promise.resolve(), placeboInvocation: 0, closed: false };
      sessions.set(key, state);
    }
    return state;
  }

  function enqueue<T>(state: SessionState, operation: () => Promise<T>): Promise<T | undefined> {
    const result = state.tail.then(operation).catch(() => undefined);
    state.tail = result.then(() => undefined);
    return result;
  }

  async function controlFailure(reason: string): Promise<void> {
    if (!receiptDirectory) return;
    try {
      await mkdir(receiptDirectory, { recursive: true, mode: 0o700 });
      await writeFile(join(receiptDirectory, "control-error.json"), `${JSON.stringify({ reason })}\n`, { mode: 0o600 });
    } catch {
      // A broken diagnostics directory must not prevent a user turn.
    }
  }

  function recordReceipt(message: string): Promise<void> {
    if (!receiptDirectory) return Promise.resolve();
    receipts = receipts.then(async () => {
      try {
        await mkdir(receiptDirectory, { recursive: true, mode: 0o700 });
        await appendFile(join(receiptDirectory, "prompt-time.jsonl"), `${JSON.stringify({ message })}\n`, { mode: 0o600 });
      } catch {
        await controlFailure("prompt receipt write failed");
      }
    });
    return receipts;
  }

  async function invokeHook(envelope: JsonObject, cwd: string): Promise<string | undefined> {
    // Defined-but-empty placebo configuration is still a control arm: never run hooks.
    if (placebo) return undefined;
    let killChild: (() => void) | undefined;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let timer: NodeJS.Timeout | undefined;
    try {
      const stdin = new TextEncoder().encode(JSON.stringify(envelope));
      if (stdin.byteLength > MAX_INPUT_BYTES) return undefined;
      const spawned = Bun.spawn([hooksBinary, "--agent", "omp"], {
        cwd,
        env: process.env,
        stdin,
        stdout: "pipe",
        stderr: "ignore",
        timeout: HOOK_TIMEOUT_MS,
        killSignal: "SIGKILL",
      });
      killChild = () => spawned.kill("SIGKILL");
      reader = spawned.stdout.getReader();
      const output = Promise.all([spawned.exited, readBounded(reader, MAX_OUTPUT_BYTES)]);
      // Also bound inherited stdout pipes: a descendant can outlive the hook process.
      const deadline = new Promise<undefined>((resolve) => {
        timer = setTimeout(() => resolve(undefined), HOOK_TIMEOUT_MS);
      });
      const result = await Promise.race([output, deadline]);
      if (!result || result[0] !== 0 || !result[1].trim()) return undefined;
      const message = objectValue(JSON.parse(result[1]))?.message;
      return typeof message === "string" ? message : undefined;
    } catch {
      return undefined;
    } finally {
      clearTimeout(timer);
      try { killChild?.(); } catch { /* Already exited. */ }
      if (reader) void reader.cancel().catch(() => undefined);
    }
  }

  async function loadPlaceboReceipts(): Promise<string[] | undefined> {
    if (!placeboSource) return undefined;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    try {
      const source = join(placeboSource, "prompt-time.jsonl");
      const info = await stat(source);
      if (!info.isFile() || info.size > MAX_PLACEBO_SOURCE_BYTES) return undefined;
      reader = Bun.file(source).stream().getReader();
      const text = await readBounded(reader, MAX_PLACEBO_SOURCE_BYTES);
      const messages: string[] = [];
      for (const line of text.split("\n")) {
        if (!line.trim()) continue;
        const message = objectValue(JSON.parse(line))?.message;
        if (typeof message !== "string" || !message || Buffer.byteLength(message) > MAX_OUTPUT_BYTES) return undefined;
        messages.push(message);
      }
      return messages;
    } catch {
      return undefined;
    } finally {
      if (reader) void reader.cancel().catch(() => undefined);
    }
  }

  async function placeboMessage(state: SessionState): Promise<string | undefined> {
    const index = state.placeboInvocation++;
    state.placeboReceipts ??= loadPlaceboReceipts();
    const original = (await state.placeboReceipts)?.[index];
    if (!original) return undefined;
    // Same estimator as scorecard-controls.py, including multibyte text.
    const padding = ".".repeat(Math.ceil(Buffer.byteLength(original) / 4) * 4);
    return forbidden.some((token) => padding.includes(token)) ? undefined : padding;
  }

  async function start(context: ExtensionContext, source: string, resetAnonymous: boolean): Promise<void> {
    try {
      const state = stateFor(context, resetAnonymous);
      state.closed = false;
      const cwd = context.cwd;
      await enqueue(state, () => invokeHook({ type: "session_start", source, cwd, session_id: state.id }, cwd));
    } catch {
      // Hosts can dispose session context while lifecycle events are being delivered.
    }
  }

  async function captureBoundary(context: ExtensionContext, close: boolean): Promise<void> {
    try {
      const state = stateFor(context);
      if (state.closed) return;
      if (close) state.closed = true;
      const cwd = context.cwd;
      // Snapshot before awaiting captures: the host can change active session meanwhile.
      const transcriptJsonl = placebo ? "" : transcript(context);
      const pending = close ? [...sessions.values()].map((session) => session.tail) : [];
      await enqueue(state, async () => {
        await Promise.all(pending);
        await receipts;
        await invokeHook({
          type: close ? "session_shutdown" : "session_checkpoint",
          reason: close ? "shutdown" : "session_transition",
          cwd, session_id: state.id, transcript_jsonl: transcriptJsonl,
        }, cwd);
      });
    } catch {
      // Capture is advisory, including when a transition is cancelled by another extension.
    }
  }

  pi.on("session_start", async (_event, context) => start(context, "startup", true));
  pi.on("session_switch", async (_event, context) => start(context, "resume", true));
  pi.on("session_branch", async (_event, context) => start(context, "resume", true));
  pi.on("session_tree", async (_event, context) => start(context, "resume", false));
  pi.on("session_before_switch", async (_event, context) => captureBoundary(context, false));
  pi.on("session_before_branch", async (_event, context) => captureBoundary(context, false));

  pi.on("before_agent_start", async (event, context) => {
    const state = stateFor(context);
    if (state.closed) return undefined;
    const cwd = context.cwd;
    return enqueue(state, async () => {
      const message = placebo
        ? await placeboMessage(state)
        : await invokeHook({ type: "prompt", cwd, session_id: state.id, prompt: event.prompt }, cwd);
      if (!message) {
        if (placebo) await controlFailure("placebo receipt unavailable or prohibited");
        return undefined;
      }
      // Receipts describe returned preparation attempts, not host commit acknowledgements.
      await recordReceipt(message);
      return {
        message: {
          customType: "dev.rusty-brain.recall",
          content: message,
          display: false,
          attribution: "agent" as const,
        },
      };
    });
  });

  pi.on("tool_result", async (event, context) => {
    try {
      const state = stateFor(context);
      if (state.closed || placebo) return;
      const cwd = context.cwd;
      const input = objectValue(event.input);
      if (!input) return;
      // Mutation capture needs paths/commands, not large file bodies or successful output.
      const toolInput = event.toolName === "write"
        ? { path: input.path, file_path: input.file_path }
        : event.toolName === "bash"
          ? { command: input.command }
          : event.toolName === "edit"
            ? { path: input.path, file_path: input.file_path, input: input.input, patch: input.patch }
            : input;
      const envelope = {
        type: "tool_result", cwd, session_id: state.id,
        tool_name: event.toolName,
        // Hashline edit is { input: string }, not { path, edits }.
        tool_input: toolInput,
        tool_response: {
          content: event.isError ? textContent(event.content, 8192) : "",
          is_error: event.isError,
        },
      };
      await enqueue(state, () => invokeHook(envelope, cwd));
    } catch {
      // Never let malformed/disposed event payloads interfere with the tool result.
    }
  });

  pi.on("session_shutdown", async (_event, context) => captureBoundary(context, true));
}
