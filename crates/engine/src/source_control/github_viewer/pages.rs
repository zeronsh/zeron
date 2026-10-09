//! Additional GitHub routes use the same authenticated, bounded CLI reads.
use super::*;

impl GitHubCli {
    async fn api_json(&self, cwd: &Path, endpoint: &str) -> Result<Value, ChangeRequestError> {
        self.viewer_json(cwd, api_args(endpoint, "application/vnd.github+json"))
            .await
    }

    async fn api_pages(
        &self,
        cwd: &Path,
        endpoint: &str,
    ) -> Result<Vec<Value>, ChangeRequestError> {
        let mut args = api_args(endpoint, "application/vnd.github+json");
        args.extend(strings(&["--paginate", "--slurp"]));
        self.viewer_json(cwd, args)
            .await?
            .as_array()
            .cloned()
            .ok_or(ChangeRequestError::Decode)
    }

    pub(super) async fn view_commit(
        &self,
        cwd: &Path,
        target: &GitHubTarget,
        sha: &str,
        page: &mut GitHubPage,
    ) -> Result<(), ChangeRequestError> {
        let endpoint = format!("repos/{}/commits/{sha}", target.repository_name());
        let metadata_endpoint = format!("{endpoint}?per_page=100");
        let comments_endpoint = format!("{endpoint}/comments?per_page=100");
        let checks_endpoint = format!("{endpoint}/check-runs?per_page=100");
        let statuses_endpoint = format!("{endpoint}/status?per_page=100");
        let (metadata, comments, checks, statuses) = tokio::try_join!(
            self.api_pages(cwd, &metadata_endpoint),
            self.api_pages(cwd, &comments_endpoint),
            self.api_pages(cwd, &checks_endpoint),
            self.api_pages(cwd, &statuses_endpoint),
        )?;
        let data = metadata.first().ok_or(ChangeRequestError::Decode)?;
        let resolved_sha = required_string(data, "sha")?;
        if !resolved_sha
            .to_ascii_lowercase()
            .starts_with(&sha.to_ascii_lowercase())
        {
            return Err(ChangeRequestError::Decode);
        }
        let message = required_string(&data["commit"], "message")?;
        let (title, body) = message.split_once('\n').unwrap_or((&message, ""));
        page.title = title.to_string();
        page.author = data["author"]["login"]
            .as_str()
            .or_else(|| data["commit"]["author"]["name"].as_str())
            .unwrap_or("Unknown author")
            .to_string();
        let date = string(&data["commit"]["author"], "date");
        page.body = format!(
            "Commit `{resolved_sha}` · {}\n\n{}",
            date.get(..10).unwrap_or(&date),
            body.trim()
        );
        if let Some(parents) = data["parents"].as_array() {
            let parents = parents
                .iter()
                .filter_map(|parent| {
                    let sha = parent["sha"].as_str()?;
                    if !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return None;
                    }
                    Some(format!(
                        "[{}](https://github.com/{}/commit/{sha})",
                        &sha[..sha.len().min(7)],
                        target.repository_name()
                    ))
                })
                .collect::<Vec<_>>();
            if !parents.is_empty() {
                page.body
                    .push_str(&format!("\n\nParents: {}", parents.join(" · ")));
            }
        }
        page.additions = count(&data["stats"], "additions");
        page.deletions = count(&data["stats"], "deletions");
        page.changed_files = metadata
            .iter()
            .map(|data| {
                data["files"]
                    .as_array()
                    .map_or(0, |files| files.len() as u32)
            })
            .sum();
        page.comments = comments
            .iter()
            .flat_map(|data| data.as_array().into_iter().flatten())
            .map(|comment| GitHubComment {
                author: comment["user"]["login"]
                    .as_str()
                    .unwrap_or("ghost")
                    .to_string(),
                body: string(comment, "body"),
                created_at: string(comment, "created_at"),
                state: String::new(),
                path: comment["path"].as_str().map(str::to_string),
            })
            .collect();
        page.checks = checks
            .iter()
            .flat_map(|data| data["check_runs"].as_array().into_iter().flatten())
            .map(|check| {
                let status = string(check, "status").to_uppercase();
                GitHubCheck {
                    name: string(check, "name"),
                    status: if status == "COMPLETED" {
                        nonempty(check, "conclusion", "status").to_uppercase()
                    } else {
                        status
                    },
                    url: string(check, "html_url"),
                }
            })
            .collect();
        page.checks.extend(
            statuses
                .iter()
                .flat_map(|data| data["statuses"].as_array().into_iter().flatten())
                .map(|status| GitHubCheck {
                    name: string(status, "context"),
                    status: string(status, "state").to_uppercase(),
                    url: string(status, "target_url"),
                }),
        );
        Ok(())
    }

    pub(super) async fn view_compare(
        &self,
        cwd: &Path,
        target: &GitHubTarget,
        range: &str,
        page: &mut GitHubPage,
    ) -> Result<(), ChangeRequestError> {
        let data = self
            .api_json(
                cwd,
                &format!(
                    "repos/{}/compare/{range}?per_page=100",
                    target.repository_name()
                ),
            )
            .await?;
        page.title = format!("Compare {range}");
        page.state = string(&data, "status").to_uppercase();
        page.body = format!(
            "{} commits · {} ahead · {} behind\n\n{}",
            count(&data, "total_commits"),
            count(&data, "ahead_by"),
            count(&data, "behind_by"),
            commit_links(&data["commits"])
        );
        if let Some(files) = data["files"].as_array() {
            page.changed_files = files.len() as u32;
            page.additions = files.iter().fold(0_u32, |total, file| {
                total.saturating_add(count(file, "additions"))
            });
            page.deletions = files.iter().fold(0_u32, |total, file| {
                total.saturating_add(count(file, "deletions"))
            });
        }
        Ok(())
    }

    pub(super) async fn view_other_page(
        &self,
        cwd: &Path,
        target: &GitHubTarget,
        url: &str,
        page: &mut GitHubPage,
    ) -> Result<(), ChangeRequestError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| ChangeRequestError::Decode)?;
        let parts: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
        if !target.repository.is_empty() {
            let route = parts.get(2).copied().unwrap_or_default();
            if route == "commits" {
                let mut endpoint = reqwest::Url::parse(&format!(
                    "https://api.github.com/repos/{}/commits",
                    target.repository_name()
                ))
                .unwrap();
                endpoint.query_pairs_mut().append_pair("per_page", "50");
                if let Some(branch) = parts.get(3) {
                    endpoint
                        .query_pairs_mut()
                        .append_pair("sha", &decode_segment(branch));
                }
                let data = self
                    .api_json(
                        cwd,
                        &format!(
                            "{}?{}",
                            endpoint.path().trim_start_matches('/'),
                            endpoint.query().unwrap_or_default()
                        ),
                    )
                    .await?;
                page.title = format!("{} · Commits", target.repository_name());
                page.body = commit_links(&data);
                return Ok(());
            }
            if matches!(route, "blob" | "tree") && parts.len() >= 4 {
                let reference = decode_segment(parts[3]);
                let path = parts[4..]
                    .iter()
                    .map(|s| decode_segment(s))
                    .collect::<Vec<_>>()
                    .join("/");
                let mut endpoint = reqwest::Url::parse(&format!(
                    "https://api.github.com/repos/{}/contents/",
                    target.repository_name()
                ))
                .unwrap();
                endpoint.path_segments_mut().unwrap().pop_if_empty();
                for part in path.split('/').filter(|s| !s.is_empty()) {
                    endpoint.path_segments_mut().unwrap().push(part);
                }
                endpoint.query_pairs_mut().append_pair("ref", &reference);
                let data = self
                    .api_json(
                        cwd,
                        &format!(
                            "{}?{}",
                            endpoint.path().trim_start_matches('/'),
                            endpoint.query().unwrap_or_default()
                        ),
                    )
                    .await?;
                page.title = if path.is_empty() {
                    format!("{} · {reference}", target.repository_name())
                } else {
                    path.clone()
                };
                if let Some(entries) = data.as_array() {
                    page.body = entries
                        .iter()
                        .map(|entry| {
                            format!(
                                "- [{}{}]({})",
                                escape_label(&string(entry, "name")),
                                if string(entry, "type") == "dir" {
                                    "/"
                                } else {
                                    ""
                                },
                                string(entry, "html_url")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                } else if data["encoding"] == "base64" {
                    use base64::Engine as _;
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(string(&data, "content").replace(['\n', '\r'], ""))
                        .map_err(|_| ChangeRequestError::Decode)?;
                    if let Ok(content) = String::from_utf8(decoded) {
                        if path.to_ascii_lowercase().ends_with(".md") {
                            page.body = content;
                        } else {
                            page.body = code_block(&path, &content);
                        }
                    } else {
                        page.body =
                            "This is a binary file. Use Open in browser to view or download it."
                                .into();
                    }
                } else {
                    page.body =
                        "This file is too large to display here. Use Open in browser to view it."
                            .into();
                }
                return Ok(());
            }
            if route == "releases" {
                let endpoint = match &parts[3..] {
                    ["tag", tag, ..] => {
                        format!("repos/{}/releases/tags/{tag}", target.repository_name())
                    }
                    ["latest"] => format!("repos/{}/releases/latest", target.repository_name()),
                    _ => format!("repos/{}/releases?per_page=30", target.repository_name()),
                };
                let data = self.api_json(cwd, &endpoint).await?;
                page.title = format!("{} · Releases", target.repository_name());
                if let Some(releases) = data.as_array() {
                    page.body = releases
                        .iter()
                        .map(|release| {
                            format!(
                                "- [{}]({})",
                                escape_label(&nonempty(release, "name", "tag_name")),
                                string(release, "html_url")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                } else {
                    page.title = nonempty(&data, "name", "tag_name");
                    page.body = string(&data, "body");
                    page.author = author(&data);
                }
                return Ok(());
            }
            if route == "actions" {
                if let ["runs", run, tail @ ..] = &parts[3..]
                    && run.parse::<u64>().is_ok_and(|n| n > 0)
                {
                    if let ["job", job] = tail
                        && job.parse::<u64>().is_ok_and(|n| n > 0)
                    {
                        let job = self
                            .api_json(
                                cwd,
                                &format!("repos/{}/actions/jobs/{job}", target.repository_name()),
                            )
                            .await?;
                        page.title = string(&job, "name");
                        page.state = nonempty(&job, "conclusion", "status").to_uppercase();
                        page.body = format!(
                            "Started {}\n\n[Workflow run](https://github.com/{}/actions/runs/{run})",
                            string(&job, "started_at"),
                            target.repository_name()
                        );
                        page.checks = job["steps"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|step| GitHubCheck {
                                name: string(step, "name"),
                                status: nonempty(step, "conclusion", "status").to_uppercase(),
                                url: String::new(),
                            })
                            .collect();
                    } else {
                        let endpoint =
                            format!("repos/{}/actions/runs/{run}", target.repository_name());
                        let job_endpoint = format!("{endpoint}/jobs?per_page=100");
                        let (run, jobs) = tokio::try_join!(
                            self.api_json(cwd, &endpoint),
                            self.api_json(cwd, &job_endpoint)
                        )?;
                        page.title = nonempty(&run, "display_title", "name");
                        page.author = run["actor"]["login"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        page.state = nonempty(&run, "conclusion", "status").to_uppercase();
                        let sha = string(&run, "head_sha");
                        page.body = format!(
                            "Workflow: {}\n\nBranch: `{}` · [Commit {}](https://github.com/{}/commit/{sha})\n\nEvent: {} · Started {}",
                            string(&run, "name"),
                            string(&run, "head_branch"),
                            &sha[..sha.len().min(7)],
                            target.repository_name(),
                            string(&run, "event"),
                            string(&run, "created_at")
                        );
                        page.checks = jobs["jobs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|job| GitHubCheck {
                                name: string(job, "name"),
                                status: nonempty(job, "conclusion", "status").to_uppercase(),
                                url: string(job, "html_url"),
                            })
                            .collect();
                    }
                } else {
                    let runs = self
                        .api_json(
                            cwd,
                            &format!(
                                "repos/{}/actions/runs?per_page=30",
                                target.repository_name()
                            ),
                        )
                        .await?;
                    page.title = format!("{} · Actions", target.repository_name());
                    page.body = runs["workflow_runs"]
                        .as_array()
                        .ok_or(ChangeRequestError::Decode)?
                        .iter()
                        .map(|run| {
                            format!(
                                "- [{}]({}) · {}",
                                escape_label(&nonempty(run, "display_title", "name")),
                                string(run, "html_url"),
                                nonempty(run, "conclusion", "status")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                return Ok(());
            }
            page.title = format!(
                "{} · {}",
                target.repository_name(),
                if route.is_empty() { "GitHub" } else { route }
            );
            page.body = format!(
                "This page isn't available in the native viewer yet.\n\n[Repository](https://github.com/{0}) · [Pull requests](https://github.com/{0}/pulls) · [Issues](https://github.com/{0}/issues) · [Commits](https://github.com/{0}/commits)\n\nUse Open in browser to access the full page.",
                target.repository_name()
            );
        } else if !target.owner.is_empty() && parts.len() == 1 {
            let data = self
                .api_json(cwd, &format!("users/{}", target.owner))
                .await?;
            let repos = self
                .api_json(
                    cwd,
                    &format!("users/{}/repos?sort=updated&per_page=30", target.owner),
                )
                .await?;
            page.title = nonempty(&data, "name", "login");
            page.body = format!(
                "{}\n\n{}",
                string(&data, "bio"),
                repos
                    .as_array()
                    .ok_or(ChangeRequestError::Decode)?
                    .iter()
                    .map(|repo| format!(
                        "- [{}]({})",
                        escape_label(&string(repo, "full_name")),
                        string(repo, "html_url")
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        } else {
            page.title = "GitHub".into();
            page.body = "This page isn't available in the native viewer yet. Use Open in browser to access it.".into();
        }
        Ok(())
    }
}

fn escape_label(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}
fn commit_links(data: &Value) -> String {
    data.as_array()
        .into_iter()
        .flatten()
        .map(|commit| {
            let message = string(&commit["commit"], "message");
            let sha = string(commit, "sha");
            format!(
                "- [{} · {}]({})",
                &sha[..sha.len().min(7)],
                escape_label(message.lines().next().unwrap_or_default()),
                string(commit, "html_url")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn decode_segment(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}
fn code_block(path: &str, content: &str) -> String {
    let fence = "`".repeat(
        content
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    let language = match path.rsplit('.').next().unwrap_or_default() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "sh" => "bash",
        "yml" => "yaml",
        "toml" => "toml",
        "json" => "json",
        _ => "",
    };
    format!("{fence}{language}\n{content}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct ApiRunner {
        requests: Mutex<Vec<ProcessRequest>>,
        wrong_sha: bool,
    }
    #[async_trait]
    impl ProcessRunner for ApiRunner {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            let endpoint = request
                .args
                .iter()
                .find(|arg| arg.starts_with("repos/"))
                .unwrap()
                .clone();
            let diff = request
                .args
                .iter()
                .any(|arg| arg == "Accept: application/vnd.github.diff");
            let data = if endpoint.contains("/comments?") {
                serde_json::json!([[{"user":{"login":"reviewer"},"body":"Looks good","created_at":"2026-10-09","path":"file.rs"}]])
            } else if endpoint.contains("/check-runs?") {
                serde_json::json!([{"check_runs":[{"name":"Windows","status":"completed","conclusion":"success","html_url":"https://github.com/acme/project/actions/runs/1"}]}])
            } else if endpoint.contains("/status?") {
                serde_json::json!([{"statuses":[{"context":"deploy","state":"pending","target_url":"https://example.com"}]}])
            } else if endpoint.contains("/contents/") {
                use base64::Engine as _;
                serde_json::json!({"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode("fn main() {}\n// ``` stays code\n")})
            } else {
                serde_json::json!([
                    {"sha": if self.wrong_sha { "abcdef1234" } else { "42926c802a837097e6a05de89d48ca7ade326658" }, "commit":{"message":"Native tabs\n\nKeep the close button on the right.","author":{"name":"Author","date":"2026-10-09T00:00:00Z"}}, "author":{"login":"contributor"}, "parents":[{"sha":"f1297c3000000000000000000000000000000000"}], "stats":{"additions":85,"deletions":66},"files":[{"filename":"shell.rs"}]},
                    {"files":[{"filename":"file.rs"}]}
                ])
            };
            self.requests.lock().unwrap().push(request);
            Ok(ProcessOutput {
                success: true,
                stdout: if diff {
                    b"diff --git a/file.rs b/file.rs\n".to_vec()
                } else {
                    serde_json::to_vec(&data).unwrap()
                },
                stderr: vec![],
                stdout_truncated: false,
            })
        }
    }
    fn provider(wrong_sha: bool) -> (GitHubCli, Arc<ApiRunner>) {
        let runner = Arc::new(ApiRunner {
            requests: Mutex::new(vec![]),
            wrong_sha,
        });
        (GitHubCli::with_runner(runner.clone()), runner)
    }

    #[tokio::test]
    async fn commit_details_and_published_patch_use_explicit_read_only_api_routes() {
        let (gh, runner) = provider(false);
        let url = "https://github.com/acme/project/commit/42926c8";
        let page = gh.view_page(Path::new("workspace"), url).await.unwrap();
        assert_eq!(page.title, "Native tabs");
        assert_eq!(page.author, "contributor");
        assert_eq!(page.changed_files, 2, "all paginated files count");
        assert_eq!((page.additions, page.deletions), (85, 66));
        assert!(page.body.contains("Keep the close button"));
        assert!(
            page.body
                .contains("https://github.com/acme/project/commit/f1297c3")
        );
        assert_eq!(page.comments[0].author, "reviewer");
        assert_eq!(page.checks[0].status, "SUCCESS");
        assert_eq!(page.checks[1].status, "PENDING");
        let diff = gh.view_diff(Path::new("workspace"), url).await.unwrap();
        assert!(diff.patch.starts_with("diff --git"));
        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        for request in requests.iter() {
            assert_eq!(request.cwd, Path::new("workspace"));
            assert_eq!(
                &request.args[..5],
                ["api", "--hostname", "github.com", "--method", "GET"]
            );
        }
        assert!(
            !requests
                .last()
                .unwrap()
                .args
                .iter()
                .any(|arg| arg == "--paginate"),
            "published diff media cannot paginate"
        );
    }

    #[tokio::test]
    async fn commit_identity_must_match_the_requested_sha() {
        let (gh, _) = provider(true);
        assert!(matches!(
            gh.view_page(
                Path::new("workspace"),
                "https://github.com/acme/project/commit/42926c8"
            )
            .await,
            Err(ChangeRequestError::Decode)
        ));
    }

    #[tokio::test]
    async fn remote_file_ref_path_and_code_fences_preserve_literal_content() {
        let (gh, runner) = provider(false);
        let page = gh
            .view_page(
                Path::new("workspace"),
                "https://github.com/acme/project/blob/feature%2Fviewer/folder/file%20name.rs#L1",
            )
            .await
            .unwrap();
        assert_eq!(page.title, "folder/file name.rs");
        assert!(page.body.starts_with("````rust\n"));
        assert!(page.body.contains("// ``` stays code"));
        let requests = runner.requests.lock().unwrap();
        assert!(
            requests[0].args.iter().any(|arg| arg
                == "repos/acme/project/contents/folder/file%20name.rs?ref=feature%2Fviewer")
        );
    }

    #[tokio::test]
    async fn unmapped_github_pages_stay_native_without_executing_the_cli() {
        let (gh, runner) = provider(false);
        let url = "https://github.com/acme/project/settings?tab=general";
        let page = gh.view_page(Path::new("workspace"), url).await.unwrap();
        assert_eq!(page.target.url(), url);
        assert!(page.body.contains("[Repository]"));
        assert!(runner.requests.lock().unwrap().is_empty());
    }
}
