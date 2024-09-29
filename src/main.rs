use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use clap::Parser;
use client::{Issue, JtClient};
use config::{Config, WorkAttribute};
use console::style;
use dialoguer::Select;
use indicatif::ProgressBar;
use reqwest::Url;
use std::{env, time::Duration};

mod client;
mod config;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(long)]
    ///Do not actually log work
    dry_run: bool,
    #[arg(long)]
    ///Fill timesheet for next week rather than current week
    next: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let config = config::load_config()?;
    let token = env::var("JIRA_TOKEN")?;
    let uri = Url::parse(&config.api_endpoint)?;

    let client = JtClient::new(&token, uri, args.dry_run);

    let now = chrono::Local::now();
    let week = if args.next {
        (now + chrono::Duration::weeks(1)).iso_week()
    } else {
        now.iso_week()
    };
    let first_day =
        NaiveDate::from_isoywd_opt(now.year(), week.week(), chrono::Weekday::Mon).unwrap();
    let done_tasks_from = first_day - chrono::Duration::days(1);

    let tasks = get_tasks(&client, done_tasks_from).await?;

    let mut work = Vec::new();
    for day in first_day.iter_days().take(5) {
        println!("{}", style(day.format("%A, %-d %B")).bold());
        let select = Select::new()
            .with_prompt("Select task")
            .items(&tasks)
            .default(0)
            .interact()
            .unwrap();
        let selected = tasks.get(select).unwrap();
        work.push((day, selected));
    }

    upload_worklogs(&client, &config, work).await
}

async fn get_tasks(client: &JtClient, done_tasks_from: NaiveDate) -> Result<Vec<Issue>> {
    let spinner = ProgressBar::new_spinner().with_message(
        style("Retrieving assigned tasks from JIRA")
            .bold()
            .to_string(),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    let tasks = client.get_assigned_issues(done_tasks_from).await?; //FIXME pass in first date
    spinner.finish_with_message(style("Assigned tasks retrieved").green().to_string());
    Ok(tasks)
}

async fn upload_worklogs(
    client: &JtClient,
    config: &Config,
    worklogs: Vec<(NaiveDate, &Issue)>,
) -> Result<()> {
    //FIXME make progress bar
    let spinner =
        ProgressBar::new_spinner().with_message(style("Logging work on Tempo").bold().to_string());
    spinner.enable_steady_tick(Duration::from_millis(100));
    for (day, log) in worklogs {
        let mut resolved_attributes = config
            .dynamic_attributes
            .iter()
            .map(|attr| {
                let pointable = serde_json::to_value(&log.fields).unwrap();
                let pointed = pointable.pointer(&attr.value).unwrap().as_str().unwrap(); //FIXME error on unwrap failure
                let mut evaluated = attr.clone();
                evaluated.value = pointed.to_owned();
                evaluated
            })
            .collect::<Vec<WorkAttribute>>();
        resolved_attributes.extend(config.static_attributes.clone());
        client
            .create_worklog(&config.worker, day, &log.key, resolved_attributes)
            .await?
    }
    spinner.finish_with_message(style("Work logged").green().bold().to_string());
    Ok(())
}
