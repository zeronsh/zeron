use std::path::Path;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use zeron_proto::{
    GitHubActivityDay, GitHubActivitySnapshot, GitHubActivityStat, GitHubActivityWeek,
};

pub(crate) const ACTIVITY_QUERY: &str = "query($from:DateTime!,$to:DateTime!,$prevFrom:DateTime!,$yearFrom:DateTime!,$opened:String!,$openedPrev:String!,$merged:String!,$mergedPrev:String!){viewer{contributionsCollection(from:$from,to:$to){contributionCalendar{totalContributions}totalPullRequestReviewContributions}previous:contributionsCollection(from:$prevFrom,to:$from){contributionCalendar{totalContributions}totalPullRequestReviewContributions}year:contributionsCollection(from:$yearFrom,to:$to){contributionCalendar{weeks{contributionDays{contributionLevel date weekday}}}}}opened:search(query:$opened,type:ISSUE,first:1){issueCount}openedPrev:search(query:$openedPrev,type:ISSUE,first:1){issueCount}merged:search(query:$merged,type:ISSUE,first:1){issueCount}mergedPrev:search(query:$mergedPrev,type:ISSUE,first:1){issueCount}}";

pub(crate) fn activity_variables(now: DateTime<Utc>) -> [(&'static str, String); 8] {
    let today = now.date_naive();
    let from = today - Duration::days(29);
    let previous_from = from - Duration::days(30);
    let current = format!("{from}..{today}");
    let previous = format!("{previous_from}..{}", from - Duration::days(1));
    [
        ("from", stamp(from)),
        ("to", stamp(today + Duration::days(1))),
        ("prevFrom", stamp(previous_from)),
        ("yearFrom", stamp(today - Duration::days(364))),
        ("opened", format!("author:@me is:pr created:{current}")),
        ("openedPrev", format!("author:@me is:pr created:{previous}")),
        (
            "merged",
            format!("author:@me is:pr is:merged merged:{current}"),
        ),
        (
            "mergedPrev",
            format!("author:@me is:pr is:merged merged:{previous}"),
        ),
    ]
}

pub(crate) fn parse_activity(payload: &Value) -> Option<GitHubActivitySnapshot> {
    let data = payload.get("data")?;
    let viewer = data.get("viewer")?;
    let current = viewer.get("contributionsCollection")?;
    let previous = viewer.get("previous")?;
    let calendar = dig(viewer, &["year", "contributionCalendar"])?;
    let total = ["contributionCalendar", "totalContributions"];
    let reviews = ["totalPullRequestReviewContributions"];
    Some(GitHubActivitySnapshot {
        contributions: stat(u32_at(current, &total)?, u32_at(previous, &total)?),
        pull_requests_opened: stat(
            u32_at(data, &["opened", "issueCount"])?,
            u32_at(data, &["openedPrev", "issueCount"])?,
        ),
        pull_requests_merged: stat(
            u32_at(data, &["merged", "issueCount"])?,
            u32_at(data, &["mergedPrev", "issueCount"])?,
        ),
        reviews: stat(u32_at(current, &reviews)?, u32_at(previous, &reviews)?),
        weeks: parse_weeks(calendar)?,
    })
}

#[derive(Deserialize)]
struct StoredActivity {
    account: String,
    snapshot: GitHubActivitySnapshot,
}

pub(crate) async fn load_stored(path: &Path, account: &str) -> Option<GitHubActivitySnapshot> {
    let stored: StoredActivity = serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()?;
    (stored.account == account).then_some(stored.snapshot)
}

pub(crate) async fn store(path: &Path, account: &str, snapshot: &GitHubActivitySnapshot) {
    let stored = serde_json::json!({ "account": account, "snapshot": snapshot });
    let Ok(bytes) = serde_json::to_vec(&stored) else {
        return;
    };
    let temp = path.with_extension("tmp");
    if tokio::fs::write(&temp, bytes).await.is_ok() {
        let _ = tokio::fs::rename(&temp, path).await;
    }
}

fn stamp(date: NaiveDate) -> String {
    format!("{date}T00:00:00Z")
}

fn stat(count: u32, previous: u32) -> GitHubActivityStat {
    let count_signed = i32::try_from(count).unwrap_or(i32::MAX);
    let previous = i32::try_from(previous).unwrap_or(i32::MAX);
    GitHubActivityStat {
        count,
        delta: count_signed.saturating_sub(previous),
    }
}

fn dig<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for key in path {
        value = value.get(*key)?;
    }
    Some(value)
}

fn u32_at(value: &Value, path: &[&str]) -> Option<u32> {
    u32::try_from(dig(value, path)?.as_u64()?).ok()
}

fn parse_weeks(calendar: &Value) -> Option<Vec<GitHubActivityWeek>> {
    let weeks = calendar.get("weeks")?.as_array()?;
    if weeks.len() > 60 {
        return None;
    }
    weeks
        .iter()
        .map(|week| {
            let days = week.get("contributionDays")?.as_array()?;
            if days.len() > 7 {
                return None;
            }
            let days = days.iter().map(parse_day).collect::<Option<_>>()?;
            Some(GitHubActivityWeek { days })
        })
        .collect()
}

fn parse_day(value: &Value) -> Option<GitHubActivityDay> {
    let date = value.get("date")?.as_str()?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let weekday = u8::try_from(u32_at(value, &["weekday"])?)
        .ok()
        .filter(|weekday| *weekday < 7)?;
    let level = match value.get("contributionLevel")?.as_str()? {
        "NONE" => 0,
        "FIRST_QUARTILE" => 1,
        "SECOND_QUARTILE" => 2,
        "THIRD_QUARTILE" => 3,
        "FOURTH_QUARTILE" => 4,
        _ => return None,
    };
    Some(GitHubActivityDay {
        date: date.to_owned(),
        level,
        weekday,
    })
}

#[cfg(test)]
pub(crate) fn test_activity_payload() -> String {
    let day = |date, level, weekday| {
        serde_json::json!({
            "date": date,
            "contributionLevel": level,
            "weekday": weekday
        })
    };
    serde_json::json!({
        "data": {
            "viewer": {
                "contributionsCollection": {
                    "contributionCalendar": { "totalContributions": 10 },
                    "totalPullRequestReviewContributions": 1
                },
                "previous": {
                    "contributionCalendar": { "totalContributions": 4 },
                    "totalPullRequestReviewContributions": 3
                },
                "year": {
                    "contributionCalendar": {
                        "weeks": [
                            { "contributionDays": [
                                day("2026-10-04", "NONE", 0),
                                day("2026-10-05", "FIRST_QUARTILE", 1)
                            ]},
                            { "contributionDays": [
                                day("2026-10-11", "SECOND_QUARTILE", 0),
                                day("2026-10-12", "FOURTH_QUARTILE", 1)
                            ]}
                        ]
                    }
                }
            },
            "opened": { "issueCount": 5 },
            "openedPrev": { "issueCount": 3 },
            "merged": { "issueCount": 2 },
            "mergedPrev": { "issueCount": 2 }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_cover_thirty_days_and_a_year() {
        let now = "2026-10-12T15:04:05Z".parse().unwrap();
        let variables = activity_variables(now);
        let value = |name: &str| {
            variables
                .iter()
                .find(|(key, _)| *key == name)
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(value("from"), "2026-09-13T00:00:00Z");
        assert_eq!(value("to"), "2026-10-13T00:00:00Z");
        assert_eq!(value("prevFrom"), "2026-08-14T00:00:00Z");
        assert_eq!(value("yearFrom"), "2025-10-13T00:00:00Z");
        assert!(value("opened").ends_with("created:2026-09-13..2026-10-12"));
        assert!(value("openedPrev").ends_with("created:2026-08-14..2026-09-12"));
        assert!(value("merged").ends_with("merged:2026-09-13..2026-10-12"));
        for (name, _) in &variables {
            assert!(ACTIVITY_QUERY.contains(&format!("${name}:")), "{name}");
        }
    }

    #[test]
    fn parse_reads_counts_and_the_calendar() {
        let payload: Value = serde_json::from_str(&test_activity_payload()).unwrap();
        let snapshot = parse_activity(&payload).unwrap();
        let counts = |stat: GitHubActivityStat| (stat.count, stat.delta);
        assert_eq!(counts(snapshot.contributions), (10, 6));
        assert_eq!(counts(snapshot.pull_requests_opened), (5, 2));
        assert_eq!(counts(snapshot.pull_requests_merged), (2, 0));
        assert_eq!(counts(snapshot.reviews), (1, -2));
        assert_eq!(snapshot.weeks.len(), 2);
        assert_eq!(snapshot.weeks[1].days[1].level, 4);
        assert_eq!(snapshot.weeks[1].days[1].weekday, 1);
    }

    #[test]
    fn rejects_incomplete_payload() {
        assert!(parse_activity(&serde_json::json!({})).is_none());
        let mut payload: Value = serde_json::from_str(&test_activity_payload()).unwrap();
        payload["data"]["viewer"]["year"]["contributionCalendar"]["weeks"][0]["contributionDays"]
            [0]["contributionLevel"] = "LOUD".into();
        assert!(parse_activity(&payload).is_none());
    }
}
