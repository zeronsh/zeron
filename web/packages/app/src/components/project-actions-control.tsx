import { useEffect, useRef, useSyncExternalStore } from "react";
import { Icon } from "@zeron/icons";
import type { ProjectAction, ProjectActionDraft, ProjectActionIcon } from "@zeron/proto";
import { fleetLocalDeviceId, useFleet, useFleetRegistry, useFleetSnapshot } from "../state/fleet";
import { useEngineSession } from "../state/session-provider";
import { uiSettings } from "../state/ui-settings";
import {
  ACTION_ICONS,
  actionIconName,
  canRun,
  preferredAction,
  projectActionContext,
  projectActionsStore,
  showActionLabel,
} from "../lib/project-actions";
import { drawerTerminalStore } from "../terminal/store";
import { PickerCard } from "./ui/PickerCard";
import { MenuHeading, MenuRow, MenuSeparator } from "./ui/MenuRows";
import { BtnDanger, BtnGhost, BtnPrimary, Dialog, DialogBody, DialogCard, DialogField, DialogTitle } from "./ui/Dialog";

/**
 * The project-Actions titlebar control — the web peer of
 * `render_project_actions_control` / the menu / the editor overlay
 * (`crates/ui/src/shell/actions_ui.rs`, final post-669d45bd geometry):
 *
 * - the preferred action's icon + name (label yielding to the titlebar's
 *   room, b1484015) with a chevron opening the menu; unavailable keeps a
 *   retryable warning segment (f4365134); an empty project offers "Add
 *   action" plus an import chevron while `zeron.json` has offers;
 * - the menu lists rows (run + edit), imports, the project-file issue and
 *   "Add action" — scrollable so 50 actions still reach Add;
 * - the editor dialog (name/command/icon/setup) and the delete confirm;
 * - a run reserves the chat's bottom-drawer tab and attaches the returned
 *   PTY (or fails the tab), remembering the preferred action id
 *   (`last_project_action_by_space_id`).
 *
 * Routing follows the ticket-24 decision: the context resolves the
 * chat-owning engine's client once (`useEngineSession` routes by the
 * chat's scoped id); params carry scoped ids and a `targetDeviceId` hint
 * that `wireParams` decodes, validates and strips at the socket.
 */
export function ProjectActionsControl({
  chatId,
  availableTitlebarWidth,
}: {
  /** The route's engine-scoped chat id. */
  readonly chatId: string;
  readonly availableTitlebarWidth: number;
}) {
  const session = useEngineSession();
  const snapshot = useFleetSnapshot();
  const registry = useFleetRegistry();
  const fleet = useFleet();
  const version = useSyncExternalStore(
    projectActionsStore.subscribe,
    projectActionsStore.getVersion,
    projectActionsStore.getVersion,
  );

  const chat = snapshot.chats.loaded ? snapshot.chats.rows.find((row) => row.id === chatId) ?? null : null;
  const context = projectActionContext({
    chat,
    spaces: snapshot.spaces.rows,
    engineKey: session === null ? "" : session.engine.baseUrl,
    client: session === null ? null : session.client,
    localDeviceId: fleetLocalDeviceId(registry, fleet.active),
  });

  useEffect(() => {
    projectActionsStore.ensure(context);
  });

  const status = projectActionsStore.activeStatus();
  if (context === null || status === null) {
    return null;
  }
  if (status.kind === "idle" || status.kind === "loading" || status.kind === "unsupported") {
    return null;
  }
  const snapshotState = projectActionsStore.visibleSnapshot();
  if (snapshotState === null) {
    return null;
  }
  const runState = canRun(status);
  const unavailable = status.kind === "unavailable";
  const preferred = preferredAction(
    snapshotState.actions,
    uiSettings.getSnapshot().lastProjectActionBySpaceId[snapshotState.spaceId] ?? null,
  );
  const hasImports = snapshotState.importableActions.length > 0;
  const hasActions = snapshotState.actions.length > 0;
  const showLabel = showActionLabel(availableTitlebarWidth);
  void version;

  const openEditor = (options: { action: ProjectAction | null; import: ProjectActionDraft | null }) => {
    projectActionsStore.openEditor(context.key, options);
  };
  const run = (action: ProjectAction) => {
    void projectActionsStore.run(context, action, drawerTerminalStore, {
      onRunStarted: (spaceId, actionId) => {
        uiSettings.updateDebounced({
          lastProjectActionBySpaceId: {
            ...uiSettings.getSnapshot().lastProjectActionBySpaceId,
            [spaceId]: actionId,
          },
        });
      },
    });
  };

  const trigger =
    preferred !== null ? (
      <button
        type="button"
        className="project-action-control"
        aria-label={`Run ${preferred.name}`}
        title={preferred.name}
      >
        <span
          className={`project-action-main ${runState ? "" : "project-action-disabled"}`}
          onClick={(event) => {
            if (runState) {
              event.stopPropagation();
              run(preferred);
            }
          }}
        >
          <Icon name={actionIconName(preferred.icon)} size={13} />
          {showLabel && <span className="project-action-name">{preferred.name}</span>}
        </span>
        <span className="project-action-chevron">
          <Icon name="altArrowDown" size={11} />
        </span>
      </button>
    ) : unavailable ? (
      <button type="button" className="project-action-control" aria-label="Actions unavailable">
        <span className="project-action-main">
          <Icon name="dangerTriangle" size={13} className="project-action-danger" />
          {showLabel && <span className="project-action-name">Actions unavailable</span>}
        </span>
      </button>
    ) : (
      <button type="button" className="project-action-control" aria-label="Add action">
        <span
          className="project-action-main"
          onClick={(event) => {
            // Consume the click — the preferred-run segment's rule: the
            // whole button is the Base UI trigger (the popover's on
            // desktop, the sheet's on phone), so an unstopped click bubbles
            // into it and re-opens the menu `openEditor` just closed — both
            // surfaces at once. The desktop's add segment opens only the
            // editor (actions_ui.rs:614).
            event.stopPropagation();
            openEditor({ action: null, import: null });
          }}
        >
          <Icon name="plus" size={13} />
          <span className="project-action-name">Add action</span>
        </span>
        {hasImports && (
          <span className="project-action-chevron">
            <Icon name="altArrowDown" size={11} />
          </span>
        )}
      </button>
    );

  return (
    <>
      <PickerCard
        open={projectActionsStore.menuOpen}
        onOpenChange={(next) => {
          if (next) {
            projectActionsStore.toggleMenu(context);
          } else {
            projectActionsStore.closeMenu();
          }
        }}
        cardClassName="popover-card project-actions-menu"
        role="menu"
        ariaLabel="Project actions"
        width={280}
        trigger={trigger}
      >
        <ProjectActionsMenuBody
          message={unavailable ? status.message : null}
          snapshotState={snapshotState}
          canRun={runState}
          onRetry={() => projectActionsStore.refresh(context)}
          onRun={run}
          onEdit={(action) => openEditor({ action, import: null })}
          onImport={(draft) => openEditor({ action: null, import: draft })}
          onAdd={() => openEditor({ action: null, import: null })}
        />
      </PickerCard>
      {projectActionsStore.editor !== null && (
        <ProjectActionEditorDialog context={context} key={context.key.spaceId} />
      )}
    </>
  );
}

function ProjectActionsMenuBody({
  message,
  snapshotState,
  canRun,
  onRetry,
  onRun,
  onEdit,
  onImport,
  onAdd,
}: {
  readonly message: string | null;
  readonly snapshotState: {
    readonly actions: readonly ProjectAction[];
    readonly importableActions: readonly ProjectActionDraft[];
    readonly projectFileIssue: string | null;
  };
  readonly canRun: boolean;
  readonly onRetry: () => void;
  readonly onRun: (action: ProjectAction) => void;
  readonly onEdit: (action: ProjectAction) => void;
  readonly onImport: (draft: ProjectActionDraft) => void;
  readonly onAdd: () => void;
}) {
  return (
    <div className="project-actions-menu-list">
      <MenuHeading>Project actions</MenuHeading>
      {message !== null && (
        <>
          <p className="project-actions-error">{message}</p>
          <MenuRow fadeKey="project-actions-retry" onClick={onRetry}>
            <Icon name="refresh" size={15} />
            <span className="menu-row-label">Retry</span>
          </MenuRow>
        </>
      )}
      {snapshotState.actions.map((action) => (
        <MenuRow
          key={action.id}
          fadeKey={`project-action-row-${action.id}`}
          disabled={!canRun}
          onClick={() => onRun(action)}
        >
          <Icon name={actionIconName(action.icon)} size={15} />
          <span className="menu-row-label">
            {action.runOnWorktreeCreate ? `${action.name} (setup)` : action.name}
          </span>
          <button
            type="button"
            className="project-action-edit"
            aria-label={`Edit ${action.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onEdit(action);
            }}
          >
            <Icon name="settingsMinimalistic" size={14} />
          </button>
        </MenuRow>
      ))}
      {snapshotState.importableActions.length > 0 && (
        <>
          <MenuSeparator />
          <MenuHeading>Import from zeron.json</MenuHeading>
          {snapshotState.importableActions.map((draft) => (
            <MenuRow
              key={draft.name}
              fadeKey={`import-project-action-${draft.name}`}
              onClick={() => onImport(draft)}
            >
              <Icon name={actionIconName(draft.icon)} size={15} />
              <span className="menu-row-label">{draft.name}</span>
            </MenuRow>
          ))}
        </>
      )}
      {snapshotState.projectFileIssue !== null && (
        <p className="project-actions-issue">{snapshotState.projectFileIssue}</p>
      )}
      <MenuSeparator />
      <MenuRow fadeKey="project-actions-add-row" onClick={onAdd}>
        <Icon name="plus" size={15} />
        <span className="menu-row-label">Add action</span>
      </MenuRow>
    </div>
  );
}

/** The editor dialog (`render_project_action_overlay`). */
function ProjectActionEditorDialog({
  context,
}: {
  readonly context: Parameters<typeof projectActionsStore.save>[0];
}) {
  const version = useSyncExternalStore(
    projectActionsStore.subscribe,
    projectActionsStore.getVersion,
    projectActionsStore.getVersion,
  );
  const editor = projectActionsStore.editor;
  const nameRef = useRef<HTMLInputElement | null>(null);
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      nameRef.current?.focus();
    }
  }, []);
  void version;
  if (editor === null) {
    return null;
  }
  const save = () => {
    projectActionsStore.save(context, () => {});
  };
  if (editor.confirmDelete) {
    return (
      <Dialog ariaLabel="Delete action?" onClose={() => projectActionsStore.setConfirmDelete(false)}>
        <DialogCard>
          <DialogTitle>Delete action?</DialogTitle>
          <DialogBody>
            {"\u201C" + editor.name.trim() + "\u201D will be permanently deleted."}
          </DialogBody>
          <div className="dialog-actions-row">
            <BtnGhost onClick={() => projectActionsStore.setConfirmDelete(false)}>Cancel</BtnGhost>
            <BtnDanger onClick={() => projectActionsStore.remove(context, { onPreferredCleared: clearPreferred })}>
              Delete
            </BtnDanger>
          </div>
        </DialogCard>
      </Dialog>
    );
  }
  const editing = editor.actionId !== null;
  return (
    <Dialog ariaLabel={editing ? "Edit action" : "Add action"} onClose={() => projectActionsStore.closeEditor()}>
      <DialogCard>
        <DialogTitle>{editing ? "Edit action" : "Add action"}</DialogTitle>
        <form
          className="dialog-form-rows"
          onSubmit={(event) => {
            event.preventDefault();
            save();
          }}
        >
          <label className="dialog-field-label" htmlFor="project-action-name">
            Name
          </label>
          <DialogField>
            <input
              ref={nameRef}
              id="project-action-name"
              type="text"
              value={editor.name}
              onChange={(event) => projectActionsStore.updateEditor({ name: event.target.value })}
              placeholder="Action name"
              spellCheck={false}
            />
          </DialogField>
          <label className="dialog-field-label" htmlFor="project-action-command">
            Command
          </label>
          <DialogField>
            <textarea
              id="project-action-command"
              rows={4}
              value={editor.command}
              onChange={(event) => projectActionsStore.updateEditor({ command: event.target.value })}
              placeholder="Command"
              spellCheck={false}
              className="project-action-command"
            />
          </DialogField>
          <span className="dialog-field-label">Icon</span>
          <div className="project-action-icon-row" role="radiogroup" aria-label="Icon">
            {ACTION_ICONS.map(({ kind, label }) => (
              <button
                key={kind}
                type="button"
                className={`project-action-icon ${editor.icon === kind ? "project-action-icon-selected" : ""}`}
                role="radio"
                aria-checked={editor.icon === kind}
                aria-label={label}
                title={label}
                onClick={() => projectActionsStore.updateEditor({ icon: kind as ProjectActionIcon })}
              >
                <Icon name={actionIconName(kind)} size={16} />
              </button>
            ))}
          </div>
          <label className="project-action-setup">
            <input
              type="checkbox"
              checked={editor.runOnWorktreeCreate}
              onChange={(event) =>
                projectActionsStore.updateEditor({ runOnWorktreeCreate: event.target.checked })
              }
            />
            Run automatically on worktree creation
          </label>
          {editor.error !== null && <p className="project-actions-error">{editor.error}</p>}
          <div className="project-action-actions-row">
            {editing && (
              <BtnGhost
                className="project-action-delete-button"
                onClick={() => projectActionsStore.setConfirmDelete(true)}
              >
                Delete action
              </BtnGhost>
            )}
            <div className="dialog-actions-row">
              <BtnGhost onClick={() => projectActionsStore.closeEditor()}>Cancel</BtnGhost>
              <BtnPrimary type="submit" disabled={editor.saving}>
                {editor.saving ? "Saving\u2026" : "Save action"}
              </BtnPrimary>
            </div>
          </div>
        </form>
      </DialogCard>
    </Dialog>
  );
}

function clearPreferred(spaceId: string, actionId: string): void {
  const current = { ...uiSettings.getSnapshot().lastProjectActionBySpaceId };
  if (current[spaceId] === actionId) {
    delete current[spaceId];
    uiSettings.updateDebounced({ lastProjectActionBySpaceId: current });
  }
}
