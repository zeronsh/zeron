/**
 * The repositories a user's GitHub connection can reach, for the "add a
 * GitHub project" flow on Cloud. The vault makes these calls itself so the
 * token never leaves it; callers get repository metadata only.
 *
 * Source of truth is the GitHub App's installations (`/user/installations`,
 * then each installation's repositories). A user with no installation gets an
 * empty list: Cloud sessions can only reach installed repositories, so
 * listing others (`/user/repos`) would offer projects that can't be cloned
 * or pushed. Every list follows `Link: rel="next"`,
 * but ONLY to `https://api.github.com` — a next link anywhere else would
 * carry the bearer token off-site — and stops at `MAX_REPOS` repositories or
 * `MAX_REQUESTS` calls, whichever comes first (one DO call must stay well
 * inside the Workers subrequest budget).
 */
import type { GithubRepoView } from "../api";
import { isObject, nonEmptyString, upstream, type FetchFn } from "./types";

export const GITHUB_API = "https://api.github.com";
export const MAX_REPOS = 1000;
const MAX_REQUESTS = 30;
const PAGE = "per_page=100";

export type RepoListing =
  | { readonly kind: "ok"; readonly repos: GithubRepoView[] }
  /** GitHub answered 401: the stored token is dead. */
  | { readonly kind: "unauthorized" }
  | { readonly kind: "failed"; readonly reason: string };

const isGithubApi = (url: string): boolean => {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" && parsed.host === "api.github.com";
  } catch {
    return false;
  }
};

/** The `rel="next"` target of a Link header, if it stays on api.github.com. */
export const nextPage = (link: string | null): string | undefined => {
  if (!link) return undefined;
  for (const part of link.split(",")) {
    const match = part.match(/<([^>]*)>(.*)/);
    const rel = match?.[2]?.match(/rel="?([^";]*)"?/)?.[1];
    if (match && rel?.split(/\s+/).includes("next")) return isGithubApi(match[1]!) ? match[1] : undefined;
  }
  return undefined;
};

export const toRepoView = (value: unknown): GithubRepoView | undefined => {
  if (!isObject(value) || !nonEmptyString(value.full_name) || !nonEmptyString(value.clone_url)) return undefined;
  const pushedAt = typeof value.pushed_at === "string" ? Date.parse(value.pushed_at) : Number.NaN;
  return {
    fullName: value.full_name,
    cloneUrl: value.clone_url,
    defaultBranch: nonEmptyString(value.default_branch) ? value.default_branch : "main",
    private: value.private === true,
    ...(nonEmptyString(value.description) ? { description: value.description } : {}),
    pushedAt: Number.isFinite(pushedAt) ? pushedAt : 0
  };
};

/** Case-insensitive substring filter on `fullName`; most recently pushed first. */
export const filterRepos = (repos: readonly GithubRepoView[], query?: string): GithubRepoView[] => {
  const needle = query?.trim().toLowerCase() ?? "";
  return repos
    .filter((repo) => !needle || repo.fullName.toLowerCase().includes(needle))
    .sort((a, b) => b.pushedAt - a.pushedAt || a.fullName.localeCompare(b.fullName));
};

export const listGithubRepos = async (accessToken: string, fetchFn: FetchFn): Promise<RepoListing> => {
  const headers = {
    accept: "application/vnd.github+json",
    authorization: `Bearer ${accessToken}`,
    "user-agent": "zeron-vault",
    "x-github-api-version": "2022-11-28"
  };
  let requests = 0;
  const repos = new Map<string, GithubRepoView>();

  /** Walk one paginated list; `take` returns false to stop early. */
  const walk = async (
    first: string,
    items: (body: unknown) => unknown[] | undefined,
    take: (item: unknown) => boolean
  ): Promise<RepoListing | undefined> => {
    let url: string | undefined = first;
    while (url && requests < MAX_REQUESTS) {
      requests++;
      const response = await upstream(fetchFn, url, { headers });
      if (!response) return { kind: "failed", reason: "timeout or network error" };
      if (response.status === 401) return { kind: "unauthorized" };
      if (!response.ok) return { kind: "failed", reason: `GitHub HTTP ${response.status}` };
      const list = items(await response.json().catch(() => undefined));
      if (!list) return { kind: "failed", reason: "unexpected GitHub response" };
      for (const item of list) if (!take(item)) return undefined;
      url = nextPage(response.headers.get("link"));
    }
    return undefined;
  };
  const addRepo = (item: unknown): boolean => {
    const repo = toRepoView(item);
    if (repo && !repos.has(repo.fullName)) repos.set(repo.fullName, repo);
    return repos.size < MAX_REPOS;
  };

  const installations: number[] = [];
  const listed = await walk(
    `${GITHUB_API}/user/installations?${PAGE}`,
    (body) => (isObject(body) && Array.isArray(body.installations) ? body.installations : undefined),
    (item) => {
      if (isObject(item) && typeof item.id === "number") installations.push(item.id);
      return true;
    }
  );
  if (listed) return listed;

  for (const id of installations) {
    if (repos.size >= MAX_REPOS) break;
    const failed = await walk(
      `${GITHUB_API}/user/installations/${id}/repositories?${PAGE}`,
      (body) => (isObject(body) && Array.isArray(body.repositories) ? body.repositories : undefined),
      addRepo
    );
    if (failed) return failed;
  }
  return { kind: "ok", repos: [...repos.values()] };
};

export type BranchListing =
  | { readonly kind: "ok"; readonly branches: string[] }
  | { readonly kind: "unauthorized" }
  /** 404: no such repository, or the connection can't see it. */
  | { readonly kind: "not_found" }
  | { readonly kind: "failed"; readonly reason: string };

/** Branch names of `owner/name` as the user token sees them (the composer's
 * base-branch picker for a new Cloud session). Same pagination rules as the
 * repository listing. */
export const listGithubBranches = async (
  accessToken: string,
  repo: string,
  fetchFn: FetchFn
): Promise<BranchListing> => {
  const headers = {
    accept: "application/vnd.github+json",
    authorization: `Bearer ${accessToken}`,
    "user-agent": "zeron-vault",
    "x-github-api-version": "2022-11-28"
  };
  const branches: string[] = [];
  let url: string | undefined = `${GITHUB_API}/repos/${repo}/branches?${PAGE}`;
  for (let requests = 0; url && requests < MAX_REQUESTS && branches.length < MAX_REPOS; requests++) {
    const response = await upstream(fetchFn, url, { headers });
    if (!response) return { kind: "failed", reason: "timeout or network error" };
    if (response.status === 401) return { kind: "unauthorized" };
    if (response.status === 404) return { kind: "not_found" };
    if (!response.ok) return { kind: "failed", reason: `GitHub HTTP ${response.status}` };
    const body: unknown = await response.json().catch(() => undefined);
    if (!Array.isArray(body)) return { kind: "failed", reason: "unexpected GitHub response" };
    for (const item of body) if (isObject(item) && nonEmptyString(item.name)) branches.push(item.name);
    url = nextPage(response.headers.get("link"));
  }
  return { kind: "ok", branches };
};
