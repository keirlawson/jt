use anyhow::Result;
use chrono::{Duration, NaiveDate};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fmt::Display};

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CreateWorklogRequest {
    worker: String,
    started: String, //FIXME date here
    time_spent_seconds: u64,
    origin_task_id: String,
    attributes: HashMap<String, WorkAttribute>,
}

#[derive(Serialize, Debug)]
struct PostApprovalRequest {
    user: User,
    period: Period,
    action: Action,
}

#[derive(Serialize, Debug)]
struct User {
    key: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Period {
    date_from: String, //FIXME date
}

#[derive(Serialize, Debug)]
enum ActionType {
    Submit, //FIXME insure correct casing of serialization
}

#[derive(Serialize, Debug)]
struct Action {
    name: ActionType,
    comment: String,
    reviewer: User,
}

#[derive(Serialize, Debug)]
struct IssueSearchRequest {
    jql: String,
    fields: Vec<String>,
}

#[derive(Deserialize)]
struct IssueSearchResponse {
    #[serde(default)]
    issues: Vec<Issue>,
}

#[derive(Deserialize)]
pub struct Issue {
    pub key: String,
    pub fields: HashMap<String, Value>,
}

impl Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let summary = self
            .fields
            .get("summary")
            .expect("Task does not contain summary field")
            .as_str()
            .expect("Summary field is not string");
        write!(f, "{} - {}", self.key, summary)
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct WorkAttribute {
    name: String,
    work_attribute_id: u64,
    value: String,
}

pub struct JtClient {
    token: String,
    internal: Client,
    base: Url,
    dry_run: bool,
}

impl JtClient {
    pub fn new(token: &str, base: Url, dry_run: bool) -> JtClient {
        JtClient {
            token: token.to_owned(),
            internal: Client::new(),
            base,
            dry_run,
        }
    }
    pub async fn create_worklog(
        &self,
        worker: &str,
        start: NaiveDate,
        task_id: &str,
        attrs: Vec<crate::config::WorkAttribute>,
    ) -> Result<()> {
        let url = self.base.join("rest/tempo-timesheets/4/worklogs").unwrap();
        let attributes = attrs.into_iter().map(|attr| {
            (
                attr.key,
                WorkAttribute {
                    name: attr.name,
                    work_attribute_id: attr.work_attribute_id,
                    value: attr.value,
                },
            )
        });
        let payload = CreateWorklogRequest {
            worker: worker.to_owned(),
            started: start.format("%Y-%m-%d").to_string(),
            time_spent_seconds: 8 * 3600,
            origin_task_id: task_id.to_owned(),
            attributes: HashMap::from_iter(attributes),
        };
        log::debug!("Create worklog request contents: {payload:?}");
        let req = self
            .internal
            .post(url)
            .json(&payload)
            .bearer_auth(self.token.clone());

        if self.dry_run {
            Ok(())
        } else {
            let res = req.send().await?.error_for_status();
            res.map(|_| ()).map_err(|e| e.into())
        }
    }

    pub async fn submit_timesheet(
        &self,
        worker: &str,
        reviewer: &str,
        monday: NaiveDate,
    ) -> Result<()> {
        let url = self
            .base
            .join("rest/tempo-timesheets/4/timesheet-approval")
            .unwrap();
        let period_start = monday - Duration::days(2); //Tempo seems to want the saturday prior
        let payload = PostApprovalRequest {
            user: User {
                key: worker.to_owned(),
            },
            period: Period {
                date_from: period_start.format("%Y-%m-%d").to_string(),
            },
            action: Action {
                name: ActionType::Submit,
                comment: String::new(),
                reviewer: User {
                    key: reviewer.to_owned(),
                },
            },
        };
        log::debug!("Create timesheet approval request contents: {payload:?}");
        let req = self
            .internal
            .post(url)
            .json(&payload)
            .bearer_auth(self.token.clone());
        if !self.dry_run {
            req.send().await?.error_for_status()?;
        }
        Ok(())
    }

    pub async fn get_assigned_issues(&self, done_tasks_from: NaiveDate) -> Result<Vec<Issue>> {
        let url = self.base.join("rest/api/2/search").unwrap();
        let done_tasks_from = done_tasks_from.format("%Y-%m-%d").to_string();
        let body = IssueSearchRequest {
            jql: format!(
                "(statusCategory NOT IN (Done) OR status CHANGED AFTER {done_tasks_from}) AND assignee IN (currentUser()) ORDER BY created DESC",
            ),
            fields: vec![String::from("*navigable")],
        };
        log::debug!("Search request contents: {body:?}");
        let res = self
            .internal
            .post(url)
            .json(&body)
            .bearer_auth(self.token.clone())
            .send()
            .await?;
        let resp = res.json::<IssueSearchResponse>().await?;
        Ok(resp.issues)
    }
}
