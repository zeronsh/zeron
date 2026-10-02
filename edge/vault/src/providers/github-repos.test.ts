import { describe, expect, it } from "vitest";
import { filterRepos, nextPage, toRepoView } from "./github-repos";

describe("github repo listing helpers", () => {
  it("follows rel=next only on api.github.com", () => {
    const next = "https://api.github.com/user/installations/1/repositories?per_page=100&page=2";
    expect(nextPage(`<${next}>; rel="next", <https://api.github.com/x?page=9>; rel="last"`)).toBe(next);
    expect(nextPage(`<https://api.github.com/x?page=1>; rel="prev", <${next}>; rel="next"`)).toBe(next);
    expect(nextPage(`<https://api.github.com/x?page=9>; rel="last"`)).toBeUndefined();
    expect(nextPage(null)).toBeUndefined();
    // A next link off-site would carry the bearer token with it.
    expect(nextPage(`<https://evil.example.com/steal?page=2>; rel="next"`)).toBeUndefined();
    expect(nextPage(`<http://api.github.com/x?page=2>; rel="next"`)).toBeUndefined();
    expect(nextPage(`<https://api.github.com.evil.example/x>; rel="next"`)).toBeUndefined();
  });

  it("maps GitHub repositories", () => {
    expect(
      toRepoView({
        full_name: "octo/zeron",
        clone_url: "https://github.com/octo/zeron.git",
        default_branch: "trunk",
        private: true,
        description: "The app",
        pushed_at: "2026-09-20T10:00:00Z",
        owner: { login: "octo" }
      })
    ).toEqual({
      fullName: "octo/zeron",
      cloneUrl: "https://github.com/octo/zeron.git",
      defaultBranch: "trunk",
      private: true,
      description: "The app",
      pushedAt: Date.parse("2026-09-20T10:00:00Z")
    });
    expect(
      toRepoView({ full_name: "octo/empty", clone_url: "https://github.com/octo/empty.git", description: null, pushed_at: null })
    ).toEqual({
      fullName: "octo/empty",
      cloneUrl: "https://github.com/octo/empty.git",
      defaultBranch: "main",
      private: false,
      pushedAt: 0
    });
    expect(toRepoView({ name: "no-full-name" })).toBeUndefined();
  });

  it("filters case-insensitively and sorts by last push", () => {
    const repo = (fullName: string, pushedAt: number) => ({
      fullName,
      cloneUrl: `https://github.com/${fullName}.git`,
      defaultBranch: "main",
      private: false,
      pushedAt
    });
    const repos = [repo("octo/alpha", 1), repo("octo/Zeron", 3), repo("zeron-org/desktop", 5), repo("octo/beta", 3)];
    expect(filterRepos(repos).map((r) => r.fullName)).toEqual([
      "zeron-org/desktop",
      "octo/beta",
      "octo/Zeron",
      "octo/alpha"
    ]);
    expect(filterRepos(repos, "  ZER ").map((r) => r.fullName)).toEqual(["zeron-org/desktop", "octo/Zeron"]);
    expect(filterRepos(repos, "nothing")).toEqual([]);
  });
});
