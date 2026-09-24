import type { EngineClient } from "@zeron/engine-client";
import { methods } from "@zeron/engine-client";
import type {
  Chat,
  ProjectAction,
  ProjectActionDraft,
  ProjectActionIcon,
  ProjectActionRun,
  ProjectActionsSnapshot,
  Space,
  TerminalSession,
} from "@zeron/proto";
import type { IconName } from "@zeron/icons";

/**
 * The web peer of the desktop's project-Actions state
 * (`crates/ui/src/project_actions.rs` + the action-context half of
 * `shell/actions_ui.rs`): viewport-local controller state for host-owned
 * project Actions, keyed per (engine, space), generation-guarded against
 * late load/mutation replies, and a retryable surface for transport
 * failures.
 *
 * Routing is the engine-local pairing model (ticket 24's decision): the
 * context captures the chat-owning engine's `EngineClient` once — the
 * session layer already routes it by the chat's scoped id — and params
 * carry the scoped `spaceId`/`chatId` plus a `targetDeviceId` hint that
 * `wireParams` decodes, validates, and strips at the socket. The engine's
 * fail-closed entry check is the assertion point; there is no relay.
 */

/** `MAX_PROJECT_ACTION_NAME_CHARS` (engine project_actions.rs). */
export const MAX_PROJECT_ACTION_NAME_CHARS = 80;

/** `MAX_PROJECT_ACTION_COMMAND_BYTES` (engine project_actions.rs). */
export const MAX_PROJECT_ACTION_COMMAND_BYTES = 16 * 1024;

/** `ACTION_LABEL_MIN_TITLEBAR_WIDTH` (b1484015). */
export const ACTION_LABEL_MIN_TITLEBAR_WIDTH = 420;

/** One cached space's identity — `ProjectActionsKey` (engine + space). */
export interface ProjectActionsKey {
  /** The owning engine's registry key (the session's `baseUrl`). */
  readonly engineKey: string;
  /** The engine-scoped space id. */
  readonly spaceId: string;
}

/** `ProjectActionsStatus` — the control's per-space state. */
export type ProjectActionsStatus =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "ready"; readonly snapshot: ProjectActionsSnapshot }
  | { readonly kind: "saving"; readonly snapshot: ProjectActionsSnapshot }
  | { readonly kind: "unavailable"; readonly snapshot: ProjectActionsSnapshot | null; readonly message: string }
  | { readonly kind: "unsupported" };

export function statusSnapshot(status: ProjectActionsStatus): ProjectActionsSnapshot | null {
  switch (status.kind) {
    case "ready":
    case "saving":
      return status.snapshot;
    case "unavailable":
      return status.snapshot;
    default:
      return null;
  }
}

export function canRun(status: ProjectActionsStatus): boolean {
  return status.kind === "ready";
}

/** `ACTION_ICONS` — the editor's icon picker, in wire order. */
export const ACTION_ICONS: readonly { readonly kind: ProjectActionIcon; readonly label: string }[] = [
  { kind: "play", label: "Play" },
  { kind: "test", label: "Test" },
  { kind: "lint", label: "Lint" },
  { kind: "configure", label: "Configure" },
  { kind: "build", label: "Build" },
  { kind: "debug", label: "Debug" },
];

/** `action_icon` — the web icon set's name for a wire icon kind. */
export function actionIconName(icon: ProjectActionIcon): IconName {
  switch (icon) {
    case "play":
      return "actionPlay";
    case "test":
      return "actionTest";
    case "lint":
      return "actionLint";
    case "configure":
      return "actionConfigure";
    case "build":
      return "actionBuild";
    case "debug":
      return "actionDebug";
  }
}

/** `show_action_label` (b1484015): label yields to the titlebar's room. */
export function showActionLabel(availableTitlebarWidth: number): boolean {
  return availableTitlebarWidth >= ACTION_LABEL_MIN_TITLEBAR_WIDTH;
}

/** `preferred_action` — the saved choice, else first non-setup, else first. */
export function preferredAction(
  actions: readonly ProjectAction[],
  preferredId: string | null,
): ProjectAction | null {
  const saved = preferredId === null ? null : actions.find((action) => action.id === preferredId) ?? null;
  if (saved !== null) {
    return saved;
  }
  return actions.find((action) => !action.runOnWorktreeCreate) ?? actions[0] ?? null;
}

/** `draft_from_action`. */
export function draftFromAction(action: ProjectAction): ProjectActionDraft {
  return {
    name: action.name,
    command: action.command,
    icon: action.icon,
    runOnWorktreeCreate: action.runOnWorktreeCreate,
  };
}

/**
 * The editor's client-side validation — `save_project_action`'s pre-flight
 * checks (the engine re-validates; these only keep the dialog honest).
 */
export function validateActionDraft(name: string, command: string): string | null {
  const trimmedName = name.trim();
  const trimmedCommand = command.trim();
  if (trimmedName.length === 0) {
    return "Action name is required";
  }
  if ([...trimmedName].length > MAX_PROJECT_ACTION_NAME_CHARS) {
    return `Action name must not exceed ${MAX_PROJECT_ACTION_NAME_CHARS} characters`;
  }
  if (trimmedCommand.length === 0) {
    return "Action command is required";
  }
  if (new TextEncoder().encode(trimmedCommand).length > MAX_PROJECT_ACTION_COMMAND_BYTES) {
    return `Action command must not exceed ${MAX_PROJECT_ACTION_COMMAND_BYTES} bytes`;
  }
  return null;
}

/** `unknown_method` — a version-skewed host hides the control. */
function isUnknownMethod(message: string): boolean {
  const lowered = message.toLowerCase();
  return lowered.includes("unknown method") || lowered.includes("unknownmethod");
}

/** The editor overlay's draft state (`ProjectActionEditor`). */
export interface ProjectActionEditorDraft {
  readonly key: ProjectActionsKey;
  readonly actionId: string | null;
  name: string;
  command: string;
  icon: ProjectActionIcon;
  runOnWorktreeCreate: boolean;
  error: string | null;
  saving: boolean;
  confirmDelete: boolean;
}

/**
 * The externally-managed terminal API a run needs — the web terminal
 * drawer's peer of `TerminalPanel::reserve_tab_for_chat` /
 * `attach_reserved_session` / `fail_reserved_tab` (panel.rs:508-564).
 */
export interface ProjectActionTerminals {
  reserveTabForChat(chatId: string, title: string): string | null;
  attachReservedSession(chatId: string, key: string, session: TerminalSession): boolean;
  failReservedTab(chatId: string, key: string, message: string): void;
}

/**
 * Everything an Action request needs, resolved once — the web peer of
 * `ProjectActionContext`/`project_action_context` (actions_ui.rs): the
 * chat-owning engine's client, the scoped ids, and the `targetDeviceId`
 * hint for the socket's wire boundary.
 */
export interface ProjectActionContext {
  readonly key: ProjectActionsKey;
  readonly chatId: string;
  readonly client: Pick<EngineClient, "call">;
  readonly targetDeviceId: string | null;
}

/**
 * Resolve the Action context for a selected chat against the fleet's
 * projected (engine-scoped) rows: the chat must name a space, the space
 * must belong to the chat's device (the same guard the engine asserts),
 * and the owning engine's client must be at hand. Null when no context
 * exists — the control hides, exactly like the desktop's.
 */
export function projectActionContext(options: {
  readonly chat: Chat | null;
  readonly spaces: readonly Space[];
  readonly engineKey: string;
  readonly client: Pick<EngineClient, "call"> | null;
  readonly localDeviceId: string | null;
}): ProjectActionContext | null {
  const chat = options.chat;
  if (chat === null || options.client === null || chat.spaceId === null || chat.spaceId === undefined) {
    return null;
  }
  const space = options.spaces.find((row) => row.id === chat.spaceId);
  if (space === undefined || space.deviceId !== chat.deviceId) {
    return null;
  }
  const targetDeviceId = options.localDeviceId === chat.deviceId ? null : chat.deviceId;
  return {
    key: { engineKey: options.engineKey, spaceId: chat.spaceId },
    chatId: chat.id,
    client: options.client,
    targetDeviceId,
  };
}

/** `project_action_params` — merge the `targetDeviceId` routing hint. */
export function projectActionParams(
  params: Record<string, unknown>,
  targetDeviceId: string | null,
): Record<string, unknown> {
  return targetDeviceId === null ? params : { ...params, targetDeviceId };
}

/** Controller-side editor updates (name/command typing, icon picks). */
export type EditorPatch = Pick<ProjectActionEditorDraft, "name" | "command" | "icon" | "runOnWorktreeCreate">;

/**
 * The viewport-local controller — `ProjectActionsController`
 * (project_actions.rs) plus the load/mutation flows of actions_ui.rs,
 * adapted to the store/`useSyncExternalStore` shape the web app uses.
 */
export class ProjectActionsStore {
  #active: ProjectActionsKey | null = null;
  #generation = 0;
  #mutationGeneration = 0;
  readonly #cache = new Map<string, ProjectActionsStatus>();
  #editor: ProjectActionEditorDraft | null = null;
  #menuOpen = false;
  #lastRunActionId: string | null = null;
  readonly #listeners = new Set<() => void>();
  #version = 0;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  getVersion = (): number => this.#version;

  #bump(): void {
    this.#version += 1;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  get active(): ProjectActionsKey | null {
    return this.#active;
  }

  get editor(): ProjectActionEditorDraft | null {
    return this.#editor;
  }

  get menuOpen(): boolean {
    return this.#menuOpen;
  }

  get lastRunActionId(): string | null {
    return this.#lastRunActionId;
  }

  activeStatus(): ProjectActionsStatus | null {
    return this.#active === null ? null : this.#cache.get(keyOf(this.#active)) ?? null;
  }

  /**
   * `visible_snapshot` (f4365134): a snapshot synthesized for a first-load
   * failure so an unavailable control keeps a visible, retryable surface.
   */
  visibleSnapshot(): ProjectActionsSnapshot | null {
    const status = this.activeStatus();
    if (status === null) {
      return null;
    }
    const snapshot = statusSnapshot(status);
    if (snapshot !== null) {
      return snapshot;
    }
    if (status.kind !== "unavailable" || this.#active === null) {
      return null;
    }
    return { spaceId: this.#active.spaceId, actions: [], importableActions: [], projectFileIssue: null };
  }

  /** `activate` — a different key invalidates everything in flight. */
  activate(key: ProjectActionsKey | null): boolean {
    if (keyOf(key) === keyOf(this.#active)) {
      return false;
    }
    this.#active = key;
    this.#generation += 1;
    this.#menuOpen = false;
    this.#editor = null;
    this.#invalidateMutation();
    this.#bump();
    return true;
  }

  #invalidateMutation(): void {
    this.#mutationGeneration += 1;
  }

  #beginMutation(): number {
    this.#invalidateMutation();
    return this.#mutationGeneration;
  }

  isCurrentMutation(key: ProjectActionsKey, generation: number): boolean {
    return keyOf(this.#active) === keyOf(key) && this.#mutationGeneration === generation;
  }

  /** `ensure`/`begin_load` — load when the key changed or nothing is cached. */
  ensure(context: ProjectActionContext | null): void {
    const key = context === null ? null : context.key;
    const changed = this.activate(key);
    const needsLoad =
      key !== null &&
      (!this.#cache.has(keyOf(key)) || (this.#cache.get(keyOf(key))?.kind ?? "idle") === "idle");
    if ((changed || needsLoad) && context !== null) {
      this.refresh(context);
    }
  }

  refresh(context: ProjectActionContext): void {
    const cacheKey = keyOf(context.key);
    this.#generation += 1;
    const generation = this.#generation;
    const status = this.#cache.get(cacheKey);
    if (status === undefined || statusSnapshot(status) === null) {
      this.#cache.set(cacheKey, { kind: "loading" });
      this.#bump();
    }
    const params = projectActionParams({ spaceId: context.key.spaceId }, context.targetDeviceId);
    context.client
      .call<ProjectActionsSnapshot>(methods.LIST_PROJECT_ACTIONS, params)
      .then((snapshot) => {
        this.#acceptLoad(context.key, generation, snapshot);
      })
      .catch((error: unknown) => {
        this.#acceptLoad(context.key, generation, error instanceof Error ? error.message : String(error));
      });
  }

  #acceptLoad(
    key: ProjectActionsKey,
    generation: number,
    result: ProjectActionsSnapshot | string,
  ): void {
    // A late reply for a superseded load never lands: only the newest
    // generation's reply is accepted (the desktop drops it the same way).
    if (keyOf(this.#active) !== keyOf(key) || this.#generation !== generation) {
      return;
    }
    this.#accept(key, generation, result);
    this.#bump();
  }

  #accept(key: ProjectActionsKey, generation: number, result: ProjectActionsSnapshot | string): void {
    if (keyOf(this.#active) !== keyOf(key) || this.#generation !== generation) {
      return;
    }
    const cacheKey = keyOf(key);
    if (typeof result === "string") {
      this.#cache.set(
        cacheKey,
        isUnknownMethod(result)
          ? { kind: "unsupported" }
          : {
              kind: "unavailable",
              snapshot: statusSnapshot(this.#cache.get(cacheKey) ?? { kind: "idle" }),
              message: result,
            },
      );
      return;
    }
    this.#cache.set(cacheKey, { kind: "ready", snapshot: result });
  }

  /** `mark_unavailable` — a run/stream failure keeps the last snapshot. */
  markUnavailable(key: ProjectActionsKey, message: string): void {
    const cacheKey = keyOf(key);
    if (isUnknownMethod(message)) {
      this.#cache.set(cacheKey, { kind: "unsupported" });
    } else {
      this.#cache.set(cacheKey, {
        kind: "unavailable",
        snapshot: statusSnapshot(this.#cache.get(cacheKey) ?? { kind: "idle" }),
        message,
      });
    }
    this.#bump();
  }

  toggleMenu(context: ProjectActionContext | null): void {
    this.#menuOpen = !this.#menuOpen;
    if (this.#menuOpen && context !== null) {
      this.refresh(context);
    }
    this.#bump();
  }

  closeMenu(): void {
    if (this.#menuOpen) {
      this.#menuOpen = false;
      this.#bump();
    }
  }

  /** `open_project_action_editor`. */
  openEditor(
    key: ProjectActionsKey,
    options: {
      readonly action: ProjectAction | null;
      readonly import: ProjectActionDraft | null;
    },
  ): void {
    this.closeMenu();
    const draft =
      options.action !== null
        ? draftFromAction(options.action)
        : options.import ?? { name: "", command: "", icon: "play" as ProjectActionIcon, runOnWorktreeCreate: false };
    this.#editor = {
      key,
      actionId: options.action?.id ?? null,
      name: draft.name,
      command: draft.command,
      icon: draft.icon,
      runOnWorktreeCreate: draft.runOnWorktreeCreate,
      error: null,
      saving: false,
      confirmDelete: false,
    };
    this.#bump();
  }

  updateEditor(patch: Partial<EditorPatch>): void {
    if (this.#editor !== null) {
      Object.assign(this.#editor, patch);
      this.#bump();
    }
  }

  /** The delete-confirm step flips; the dialog itself stays one draft. */
  setConfirmDelete(confirm: boolean): void {
    if (this.#editor !== null && this.#editor.confirmDelete !== confirm) {
      this.#editor.confirmDelete = confirm;
      this.#bump();
    }
  }

  closeEditor(): void {
    if (this.#editor !== null) {
      this.#editor = null;
      this.#bump();
    }
  }

  /** `save_project_action` — upsert with mutation-generation gating. */
  save(context: ProjectActionContext, onSaved: () => void): void {
    const editor = this.#editor;
    if (editor === null || editor.saving || keyOf(editor.key) !== keyOf(context.key)) {
      if (editor !== null && keyOf(editor.key) !== keyOf(context.key)) {
        editor.error = "The selected project changed";
        this.#bump();
      }
      return;
    }
    const name = editor.name.trim();
    const command = editor.command.trim();
    const error = validateActionDraft(name, command);
    if (error !== null) {
      editor.error = error;
      this.#bump();
      return;
    }
    const cacheKey = keyOf(context.key);
    const snapshot = statusSnapshot(this.#cache.get(cacheKey) ?? { kind: "idle" });
    if (snapshot !== null) {
      this.#cache.set(cacheKey, { kind: "saving", snapshot });
    }
    editor.saving = true;
    editor.error = null;
    const mutation = this.#beginMutation();
    this.#bump();
    const params = projectActionParams(
      { spaceId: context.key.spaceId, actionId: editor.actionId, action: { name, command, icon: editor.icon, runOnWorktreeCreate: editor.runOnWorktreeCreate } },
      context.targetDeviceId,
    );
    context.client
      .call<ProjectActionsSnapshot>(methods.UPSERT_PROJECT_ACTION, params)
      .then((reply) => {
        this.#finishMutation(context.key, mutation, () => {
          this.#cache.set(cacheKey, { kind: "ready", snapshot: reply });
          if (this.#editor === editor) {
            this.#editor = null;
          }
          onSaved();
        });
      })
      .catch((failure: unknown) => {
        const message = failure instanceof Error ? failure.message : String(failure);
        this.#finishMutation(context.key, mutation, () => {
          this.#cache.set(cacheKey, { kind: "unavailable", snapshot: null, message });
          if (this.#editor === editor) {
            editor.saving = false;
            editor.error = message;
          }
        });
      });
  }

  /** `delete_project_action` — the same gating, plus the preferred-id clear. */
  remove(
    context: ProjectActionContext,
    options: { readonly onPreferredCleared: (spaceId: string, actionId: string) => void },
  ): void {
    const editor = this.#editor;
    if (editor === null || editor.saving || editor.actionId === null) {
      if (editor !== null && editor.actionId === null) {
        this.#editor = null;
        this.#bump();
      }
      return;
    }
    const actionId = editor.actionId;
    const cacheKey = keyOf(context.key);
    editor.saving = true;
    const mutation = this.#beginMutation();
    this.#bump();
    const params = projectActionParams(
      { spaceId: context.key.spaceId, actionId },
      context.targetDeviceId,
    );
    context.client
      .call<ProjectActionsSnapshot>(methods.DELETE_PROJECT_ACTION, params)
      .then((reply) => {
        this.#finishMutation(context.key, mutation, () => {
          this.#cache.set(cacheKey, { kind: "ready", snapshot: reply });
          options.onPreferredCleared(context.key.spaceId, actionId);
          if (this.#editor === editor) {
            this.#editor = null;
          }
        });
      })
      .catch((failure: unknown) => {
        const message = failure instanceof Error ? failure.message : String(failure);
        this.#finishMutation(context.key, mutation, () => {
          this.#cache.set(cacheKey, { kind: "unavailable", snapshot: null, message });
          if (this.#editor === editor) {
            editor.saving = false;
            editor.confirmDelete = false;
            editor.error = message;
          }
        });
      });
  }

  #finishMutation(key: ProjectActionsKey, mutation: number, apply: () => void): void {
    if (!this.isCurrentMutation(key, mutation)) {
      return; // A newer mutation or a project switch invalidated this reply.
    }
    apply();
    this.#bump();
  }

  /**
   * `run_project_action` — reserve the drawer tab, run on the owning
   * engine, attach the returned PTY (or fail the tab), remember the
   * preferred action id. Returns the run's promise for tests.
   */
  run(
    context: ProjectActionContext,
    action: ProjectAction,
    terminals: ProjectActionTerminals,
    options: { readonly onRunStarted: (spaceId: string, actionId: string) => void },
  ): Promise<void> {
    const status = this.activeStatus();
    if (status === null || !canRun(status)) {
      return Promise.resolve();
    }
    this.closeMenu();
    const tabKey = terminals.reserveTabForChat(context.chatId, action.name);
    const params = projectActionParams(
      { spaceId: context.key.spaceId, chatId: context.chatId, actionId: action.id, cols: 80, rows: 24 },
      context.targetDeviceId,
    );
    return context.client
      .call<ProjectActionRun>(methods.RUN_PROJECT_ACTION, params)
      .then((run) => {
        if (tabKey === null || !terminals.attachReservedSession(context.chatId, tabKey, run.terminal)) {
          // The tab was closed while the run was in flight — release the PTY
          // (desktop run_project_action's !attached path).
          void context.client
            .call(methods.CLOSE_TERMINAL, projectActionParams({ terminalId: run.terminal.id }, context.targetDeviceId))
            .catch(() => {});
        }
        this.#lastRunActionId = action.id;
        options.onRunStarted(context.key.spaceId, action.id);
        this.#bump();
      })
      .catch((failure: unknown) => {
        const message = failure instanceof Error ? failure.message : String(failure);
        if (tabKey !== null) {
          terminals.failReservedTab(context.chatId, tabKey, message);
        }
        this.markUnavailable(context.key, message);
      });
  }
}

function keyOf(key: ProjectActionsKey | null): string {
  return key === null ? "" : `${key.engineKey}\n${key.spaceId}`;
}

/** The app-global controller — the `Shell.project_actions` field's peer. */
export const projectActionsStore = new ProjectActionsStore();
