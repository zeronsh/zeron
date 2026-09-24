import { useEffect, useRef, type ReactNode } from "react";
import { Icon } from "@zeron/icons";
import type { SlashCommand } from "@zeron/proto";
import { MenuRow } from "../ui/MenuRows";
import { MenuScrollbar } from "../ui/Scrollbar";
import { SkeletonRows } from "../ui/Skeleton";
import { slashDescription } from "../../lib/slash";
import type { CompletionToken } from "../../lib/mentions";
import { CompletionPopup } from "./mention-popup";

/**
 * The `/` slash-command completion popup — `render_slash_popup`
 * (composer.rs:5513-5660): the mention popup's exact frame, card, scroll
 * host and 320/312 height budget, with command rows instead of file rows.
 * The candidate list is fetched once per harness and filtered locally per
 * keystroke — no RPC, no debounce, no skeleton churn while typing.
 */

export interface SlashPopupProps {
  readonly token: CompletionToken;
  /** The cached commands for the resolved harness (may be empty). */
  readonly commands: readonly SlashCommand[];
  /** Ranked indices into `commands` for the current query. */
  readonly filtered: readonly number[];
  readonly active: number | null;
  readonly loading: boolean;
  readonly error: string | null;
  /** Clicking (or Enter/Tab on) row `ix` of the FILTERED list accepts it. */
  readonly onAccept: (rowIx: number) => void;
  readonly onDismiss: () => void;
  readonly onCardMouseDown: () => void;
}

/** The slash popup: skeleton / error / empty / rows (composer.rs:5542-5652). */
export function SlashPopup(props: SlashPopupProps) {
  const listRef = useRef<HTMLDivElement | null>(null);
  // A fresh query/reopen restarts the row stack at the top (composer.rs:5447).
  useEffect(() => {
    if (listRef.current !== null) {
      listRef.current.scrollTop = 0;
    }
  }, [props.filtered]);

  let body: ReactNode;
  if (props.loading && props.commands.length === 0) {
    body = <SkeletonRows count={3} />;
  } else if (props.error !== null) {
    body = <div className="composer-completion-error" role="alert">{props.error}</div>;
  } else if (props.filtered.length === 0) {
    body = (
      <div className="composer-completion-empty">
        {props.commands.length === 0 ? "This agent has no slash commands" : "No matching commands"}
      </div>
    );
  } else {
    body = (
      <div className="composer-completion-host">
        <div className="composer-completion-list" ref={listRef}>
          {props.filtered.map((commandIx, rowIx) => {
            const command = props.commands[commandIx];
            if (command === undefined) {
              return null;
            }
            return (
              <SlashRow
                key={command.name}
                command={command}
                selected={props.active === rowIx}
                onAccept={() => props.onAccept(rowIx)}
              />
            );
          })}
        </div>
        <MenuScrollbar scrollRef={listRef} />
      </div>
    );
  }

  return (
    <CompletionPopup
      onDismiss={props.onDismiss}
      onCardMouseDown={props.onCardMouseDown}
      ariaLabel="Slash commands"
    >
      {body}
    </CompletionPopup>
  );
}

/** One command row (composer.rs:5589-5626): the 14px command glyph, the
 * `/{name}` (12.5px, medium), and the description (12px, truncated). */
function SlashRow({
  command,
  selected,
  onAccept,
}: {
  readonly command: SlashCommand;
  readonly selected: boolean;
  readonly onAccept: () => void;
}) {
  return (
    <MenuRow
      fadeKey={`slash-result-${command.name}`}
      selected={selected}
      onClick={onAccept}
      className="composer-completion-row"
    >
      <span className="composer-completion-row-icon">
        <Icon name="command" size={14} className="slash-row-command" />
      </span>
      <span className="slash-row-name">/{command.name}</span>
      <span className="slash-row-description">{slashDescription(command)}</span>
    </MenuRow>
  );
}
