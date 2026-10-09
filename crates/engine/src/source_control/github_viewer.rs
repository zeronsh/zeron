//! Native GitHub viewer reads share the badge provider's bounded, noninteractive runner.
use super::*;
use serde_json::Value;
use zeron_proto::{
    GitHubCheck, GitHubComment, GitHubDiff, GitHubListItem, GitHubPage, GitHubResource,
    GitHubTarget,
};

const VIEW_LIMIT: usize = 4 * 1024 * 1024;
mod pages;

impl GitHubCli {
    async fn viewer_command(
        &self,
        cwd: &Path,
        args: Vec<String>,
    ) -> Result<ProcessOutput, ChangeRequestError> {
        let output = self
            .runner
            .run(ProcessRequest {
                program: "gh".into(),
                args,
                cwd: cwd.to_owned(),
                env: vec![
                    ("GH_PROMPT_DISABLED".into(), "1".into()),
                    ("GH_HOST".into(), "github.com".into()),
                ],
                timeout: GITHUB_TIMEOUT,
                output_limit: VIEW_LIMIT,
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        Ok(output)
    }

    async fn viewer_json(
        &self,
        cwd: &Path,
        args: Vec<String>,
    ) -> Result<Value, ChangeRequestError> {
        let output = self.viewer_command(cwd, args).await?;
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)
    }

    pub async fn view_page(&self, cwd: &Path, url: &str) -> Result<GitHubPage, ChangeRequestError> {
        let target = if url.is_empty() {
            let source = ChangeRequestResolver::new().inspect_checkout(cwd).await?;
            let branch = source.branch;
            if branch.host.as_deref() != Some("github.com") {
                return Err(ChangeRequestError::UnsupportedRepository);
            }
            let owner = branch
                .owner
                .ok_or(ChangeRequestError::UnsupportedRepository)?;
            let repository = branch
                .repository
                .ok_or(ChangeRequestError::UnsupportedRepository)?;
            GitHubTarget::from_url(&format!("https://github.com/{owner}/{repository}"))
                .ok_or(ChangeRequestError::UnsupportedRepository)?
        } else {
            GitHubTarget::from_url(url).ok_or(ChangeRequestError::UnsupportedRepository)?
        };
        let repository = target.repository_name();
        let mut page = GitHubPage {
            target: target.clone(),
            title: repository.clone(),
            body: String::new(),
            author: String::new(),
            state: String::new(),
            draft: false,
            base_ref: String::new(),
            head_ref: String::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            items: vec![],
            comments: vec![],
            checks: vec![],
        };
        match &target.resource {
            GitHubResource::Repository | GitHubResource::PullRequests | GitHubResource::Issues => {
                if target.resource == GitHubResource::Repository {
                    let repo = self
                        .viewer_json(
                            cwd,
                            strings(&[
                                "repo",
                                "view",
                                &repository,
                                "--json",
                                "nameWithOwner,description,url",
                            ]),
                        )
                        .await?;
                    page.body = string(&repo, "description");
                }
                let kind = if target.resource == GitHubResource::Issues {
                    "issue"
                } else {
                    "pr"
                };
                let fields = if kind == "pr" {
                    "number,title,url,state,author,updatedAt,isDraft"
                } else {
                    "number,title,url,state,author,updatedAt"
                };
                let data = self
                    .viewer_json(
                        cwd,
                        strings(&[
                            kind,
                            "list",
                            "--repo",
                            &repository,
                            "--state",
                            "all",
                            "--limit",
                            "50",
                            "--json",
                            fields,
                        ]),
                    )
                    .await?;
                let items = data.as_array().ok_or(ChangeRequestError::Decode)?;
                page.items = items
                    .iter()
                    .map(|item| {
                        Ok(GitHubListItem {
                            number: item["number"]
                                .as_u64()
                                .filter(|n| *n > 0)
                                .ok_or(ChangeRequestError::Decode)?,
                            title: required_string(item, "title")?,
                            url: required_string(item, "url")?,
                            state: string(item, "state"),
                            author: author(item),
                            updated_at: string(item, "updatedAt"),
                            draft: item["isDraft"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect::<Result<_, ChangeRequestError>>()?;
            }
            GitHubResource::PullRequest(n) | GitHubResource::Issue(n) => {
                let pr = matches!(target.resource, GitHubResource::PullRequest(_));
                let fields = if pr {
                    "number,title,body,author,state,isDraft,baseRefName,headRefName,additions,deletions,changedFiles,comments,reviews,statusCheckRollup"
                } else {
                    "number,title,body,author,state,comments"
                };
                let data = self
                    .viewer_json(
                        cwd,
                        strings(&[
                            if pr { "pr" } else { "issue" },
                            "view",
                            &n.to_string(),
                            "--repo",
                            &repository,
                            "--json",
                            fields,
                        ]),
                    )
                    .await?;
                if data["number"].as_u64() != Some(*n) {
                    return Err(ChangeRequestError::Decode);
                }
                page.title = required_string(&data, "title")?;
                page.body = string(&data, "body");
                page.author = author(&data);
                page.state = string(&data, "state");
                page.draft = data["isDraft"].as_bool().unwrap_or(false);
                page.base_ref = string(&data, "baseRefName");
                page.head_ref = string(&data, "headRefName");
                page.additions = count(&data, "additions");
                page.deletions = count(&data, "deletions");
                page.changed_files = count(&data, "changedFiles");
                for (key, date_key) in [("comments", "createdAt"), ("reviews", "submittedAt")] {
                    if let Some(items) = data[key].as_array() {
                        page.comments.extend(items.iter().map(|item| GitHubComment {
                            author: author(item),
                            body: string(item, "body"),
                            created_at: string(item, date_key),
                            state: if key == "reviews" {
                                string(item, "state")
                            } else {
                                String::new()
                            },
                            path: None,
                        }));
                    }
                }
                page.comments
                    .sort_by(|a, b| a.created_at.cmp(&b.created_at));
                page.checks = data["statusCheckRollup"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|check| {
                        let status = string(check, "status");
                        GitHubCheck {
                            name: nonempty(check, "name", "context"),
                            status: if status == "COMPLETED" {
                                nonempty(check, "conclusion", "status")
                            } else if status.is_empty() {
                                string(check, "state")
                            } else {
                                status
                            },
                            url: nonempty(check, "detailsUrl", "targetUrl"),
                        }
                    })
                    .collect();
            }
            GitHubResource::Commit(sha) => self.view_commit(cwd, &target, sha, &mut page).await?,
            GitHubResource::Compare(range) => {
                self.view_compare(cwd, &target, range, &mut page).await?
            }
            GitHubResource::Page(url) => self.view_other_page(cwd, &target, url, &mut page).await?,
        }
        Ok(page)
    }

    /// Fetch GitHub's published patch lazily. It must never be substituted with
    /// the selected checkout's working-tree diff (forks and stale branches differ).
    pub async fn view_diff(&self, cwd: &Path, url: &str) -> Result<GitHubDiff, ChangeRequestError> {
        let target =
            GitHubTarget::from_url(url).ok_or(ChangeRequestError::UnsupportedRepository)?;
        let args = match &target.resource {
            GitHubResource::PullRequest(n) => strings(&[
                "pr",
                "diff",
                &n.to_string(),
                "--repo",
                &target.repository_name(),
                "--color",
                "never",
            ]),
            GitHubResource::Commit(sha) => api_args(
                &format!("repos/{}/commits/{sha}", target.repository_name()),
                "application/vnd.github.diff",
            ),
            GitHubResource::Compare(range) => api_args(
                &format!("repos/{}/compare/{range}", target.repository_name()),
                "application/vnd.github.diff",
            ),
            _ => return Err(ChangeRequestError::UnsupportedRepository),
        };
        let output = self.viewer_command(cwd, args).await?;
        Ok(GitHubDiff {
            patch: String::from_utf8_lossy(&output.stdout).into_owned(),
            truncated: output.stdout_truncated,
        })
    }
}

fn api_args(endpoint: &str, accept: &str) -> Vec<String> {
    strings(&[
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        endpoint,
        "--header",
        &format!("Accept: {accept}"),
    ])
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}
fn required_string(value: &Value, key: &str) -> Result<String, ChangeRequestError> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or(ChangeRequestError::Decode)
}
fn nonempty(value: &Value, first: &str, second: &str) -> String {
    let result = string(value, first);
    if result.is_empty() {
        string(value, second)
    } else {
        result
    }
}
fn author(value: &Value) -> String {
    value["author"]["login"]
        .as_str()
        .unwrap_or("ghost")
        .to_string()
}
fn count(value: &Value, key: &str) -> u32 {
    value[key].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Runner {
        output: ProcessOutput,
        requests: Mutex<Vec<ProcessRequest>>,
    }
    #[async_trait]
    impl ProcessRunner for Runner {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            self.requests.lock().unwrap().push(request);
            Ok(self.output.clone())
        }
    }
    fn provider(json: Value) -> (GitHubCli, Arc<Runner>) {
        let runner = Arc::new(Runner {
            output: ProcessOutput {
                success: true,
                stdout: serde_json::to_vec(&json).unwrap(),
                stderr: vec![],
                stdout_truncated: false,
            },
            requests: Mutex::new(vec![]),
        });
        (GitHubCli::with_runner(runner.clone()), runner)
    }

    #[tokio::test]
    async fn published_pr_uses_explicit_repository_and_normalizes_checks_and_reviews() {
        let (gh, runner) = provider(serde_json::json!({
            "number": 42, "title": "Native viewer", "author": {"login":"contributor"}, "state":"OPEN",
            "comments":[{"author":{"login":"a"},"body":"Comment","createdAt":"2026-01-02"}],
            "reviews":[{"author":{"login":"b"},"body":"Looks good","state":"APPROVED","submittedAt":"2026-01-01"}],
            "statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/acme/repo/actions/runs/1"},
                {"context":"deploy","state":"PENDING","targetUrl":"https://example.com"}]
        }));
        let page = gh
            .view_page(
                Path::new("checkout"),
                "https://github.com/acme/repo/pull/42",
            )
            .await
            .unwrap();
        assert_eq!(page.comments[0].state, "APPROVED");
        assert_eq!(page.checks[0].status, "SUCCESS");
        assert_eq!(page.checks[1].status, "PENDING");
        let requests = runner.requests.lock().unwrap();
        assert_eq!(
            &requests[0].args[..5],
            ["pr", "view", "42", "--repo", "acme/repo"]
        );
        assert!(
            requests[0]
                .env
                .contains(&("GH_HOST".into(), "github.com".into()))
        );
        assert_eq!(requests[0].output_limit, VIEW_LIMIT);
    }

    #[tokio::test]
    async fn rejects_foreign_hosts_before_process_execution() {
        let (gh, runner) = provider(Value::Null);
        assert!(
            gh.view_page(Path::new("checkout"), "https://evil.test/acme/repo/pull/1")
                .await
                .is_err()
        );
        assert!(
            gh.view_diff(
                Path::new("checkout"),
                "https://github.com/acme/repo/issues/1"
            )
            .await
            .is_err()
        );
        assert!(runner.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn truncated_metadata_is_rejected_but_patches_remain_readable() {
        let runner = Arc::new(Runner {
            output: ProcessOutput {
                success: true,
                stdout: b"diff --git a/a b/a\n".to_vec(),
                stderr: vec![],
                stdout_truncated: true,
            },
            requests: Mutex::new(vec![]),
        });
        let gh = GitHubCli::with_runner(runner);
        let url = "https://github.com/acme/repo/pull/42";
        assert!(matches!(
            gh.view_page(Path::new("checkout"), url).await,
            Err(ChangeRequestError::Decode)
        ));
        assert!(
            gh.view_diff(Path::new("checkout"), url)
                .await
                .unwrap()
                .truncated
        );
    }
}
