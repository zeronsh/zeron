use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityStat {
    pub count: u32,
    pub delta: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityDay {
    pub date: String,
    pub level: u8,
    pub weekday: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivityWeek {
    pub days: Vec<GitHubActivityDay>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActivitySnapshot {
    pub contributions: GitHubActivityStat,
    pub pull_requests_opened: GitHubActivityStat,
    pub pull_requests_merged: GitHubActivityStat,
    pub reviews: GitHubActivityStat,
    pub weeks: Vec<GitHubActivityWeek>,
}
