import {
  MAX_WORKSPACE_IMAGE_BYTES,
  WORKSPACE_IMAGE_CHUNK_BYTES,
  type ListWorkspaceDirectoryRequest,
  type ReadWorkspaceFileRequest,
  type ReadWorkspaceImageRequest,
  type SearchWorkspaceFilesRequest,
  type WatchWorkspaceFilesRequest,
  type WorkspaceDirectoryPage,
  type WorkspaceFileChanges,
  type WorkspaceFileSearchMatch,
  type WorkspaceFileText,
  type WorkspaceGitStatusFrame,
  type WorkspaceImageChunk,
  type WriteWorkspaceFileOutcome,
  type WriteWorkspaceFileRequest,
} from "@zeron/proto";
import { methods, RpcError, type EngineClient, type WatchHandle, type WatchHandlers } from "@zeron/engine-client";

/**
 * The workspace files RPC surface (a port of the desktop's
 * `WorkspaceFilesClient`, crates/ui/src/files/client.rs) over the web
 * client's `EngineClient.call`. The target pins one checkout — the pane's
 * Files surface targets the ACTIVE CHAT (`{ chatId }`, the desktop's
 * `FilesRequestContext::for_chat`); the engine resolves it
 * (crates/engine/src/workspace_files.rs resolve_target).
 */

/** The slice of EngineClient the files surface needs (structural, testable). */
export interface FilesCaller {
  call<T>(method: string, params?: unknown): Promise<T>;
}

/**
 * Wire target fields, flattened into every request (serde flatten parity).
 * Exactly one of `chatId` / `spaceId` — the engine's `resolve_target`
 * (workspace_files.rs:221) scopes a chat target to the active chat's
 * checkout and a space target to the space folder (plus optional
 * `checkoutPath`).
 */
export interface FilesTarget {
  readonly chatId?: string | null;
  readonly spaceId?: string | null;
  readonly checkoutPath?: string | null;
  /**
   * Routing hint for the space-owning device (the desktop context's
   * `target_device_id`, merged into every request by `request_params`):
   * the socket's `wireParams` decodes and strips it, and the engine fails
   * closed when it names anything but its own device. Absent for chat
   * targets (`FilesRequestContext::for_chat` keeps it `None`).
   */
  readonly targetDeviceId?: string | null;
}

export interface WorkspaceImage {
  readonly mimeType: string;
  readonly bytes: Uint8Array;
}

export class WorkspaceFilesClient {
  readonly #caller: FilesCaller;
  readonly #target: FilesTarget;

  constructor(caller: FilesCaller, target: FilesTarget) {
    this.#caller = caller;
    this.#target = target;
  }

  listDirectory(directory: string, includeIgnored: boolean, cursor?: string): Promise<WorkspaceDirectoryPage> {
    const request: ListWorkspaceDirectoryRequest = {
      ...this.#target,
      directory,
      includeIgnored,
      ...(cursor !== undefined ? { cursor } : {}),
    };
    return this.#caller.call(methods.LIST_WORKSPACE_DIRECTORY, request);
  }

  search(query: string, includeIgnored: boolean, limit = 200): Promise<WorkspaceFileSearchMatch[]> {
    const request: SearchWorkspaceFilesRequest = { ...this.#target, query, includeIgnored, limit };
    return this.#caller.call(methods.SEARCH_WORKSPACE_FILES, request);
  }

  readFile(path: string): Promise<WorkspaceFileText> {
    const request: ReadWorkspaceFileRequest = { ...this.#target, path };
    return this.#caller.call(methods.READ_WORKSPACE_FILE, request);
  }

  writeFile(
    request: Omit<WriteWorkspaceFileRequest, "spaceId" | "chatId" | "checkoutPath">,
  ): Promise<WriteWorkspaceFileOutcome> {
    return this.#caller.call(methods.WRITE_WORKSPACE_FILE, { ...this.#target, ...request });
  }

  /** The watch request body for `EngineClient.watch(WATCH_WORKSPACE_FILES, …)`. */
  watchParams(): WatchWorkspaceFilesRequest {
    return { ...this.#target };
  }

  /** Subscribe the workspace change stream on a connected engine client. */
  watchFiles(client: EngineClient, handlers: WatchHandlers<WorkspaceFileChanges>): WatchHandle {
    return client.watch(methods.WATCH_WORKSPACE_FILES, this.watchParams(), handlers);
  }

  /**
   * The shared remote-safe Git status stream (`WatchWorkspaceGitStatus`,
   * b25dd404): `{ chatId }` → `WorkspaceGitStatusFrame`s, computed on the
   * OWNING engine — status only, never content or patches.
   */
  watchGitStatus(
    client: EngineClient,
    handlers: WatchHandlers<WorkspaceGitStatusFrame>,
  ): WatchHandle {
    return client.watch(methods.WATCH_WORKSPACE_GIT_STATUS, this.watchParams(), handlers);
  }

  /**
   * Read a whole image through the chunked image RPC, with the desktop's
   * frame validation (client.rs read_image): identity, size, and offsets
   * must stay consistent across chunks or the read fails.
   */
  async readImage(path: string, expectedCheckoutId: string): Promise<WorkspaceImage> {
    if (expectedCheckoutId.length === 0) {
      throw new Error("Workspace checkout identity unavailable");
    }
    const parts: Uint8Array[] = [];
    let offset = 0;
    let expectedContentHash: string | null = null;
    let mimeType: string | null = null;
    let size: number | null = null;
    const maxChunks = Math.floor(MAX_WORKSPACE_IMAGE_BYTES / WORKSPACE_IMAGE_CHUNK_BYTES);
    for (let chunkIndex = 0; chunkIndex <= maxChunks; chunkIndex += 1) {
      const request: ReadWorkspaceImageRequest = {
        ...this.#target,
        path,
        expectedCheckoutId,
        offset,
        expectedContentHash,
      };
      const chunk = await this.#caller.call<WorkspaceImageChunk>(methods.READ_WORKSPACE_IMAGE, request);
      if (
        chunk.checkoutId !== expectedCheckoutId ||
        chunk.contentHash.length === 0 ||
        chunk.size > MAX_WORKSPACE_IMAGE_BYTES ||
        chunk.data.length > Math.ceil(WORKSPACE_IMAGE_CHUNK_BYTES / 3) * 4 ||
        (expectedContentHash !== null && expectedContentHash !== chunk.contentHash) ||
        (mimeType !== null && mimeType !== chunk.mimeType) ||
        (size !== null && size !== chunk.size)
      ) {
        throw new Error("Image identity or size changed");
      }
      const part = decodeBase64(chunk.data);
      if (
        part.length === 0 ||
        offset + part.length !== chunk.nextOffset ||
        chunk.nextOffset > chunk.size ||
        chunk.done !== (chunk.nextOffset === chunk.size)
      ) {
        throw new Error("Invalid image chunk offset");
      }
      parts.push(part);
      if (chunk.done) {
        return { mimeType: chunk.mimeType, bytes: concat(parts, chunk.size) };
      }
      offset = chunk.nextOffset;
      expectedContentHash = chunk.contentHash;
      mimeType = chunk.mimeType;
      size = chunk.size;
    }
    throw new Error("Image chunk limit exceeded");
  }
}

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/** Error copy for the files surface (RpcError kind → honest message). */
export function describeFilesError(error: unknown): string {
  if (error instanceof RpcError) {
    if (error.kind === "transport") {
      return "Engine is offline; reconnecting";
    }
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

function concat(parts: readonly Uint8Array[], size: number): Uint8Array {
  const out = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}
