use anyhow::{bail, Result};
use chrono::{Datelike, NaiveDate, TimeDelta};
use clap::Parser;
use client::{Issue, JtClient};
use config::{Config, WorkAttribute};
use console::style;
use dialoguer::{Input, Select};
use indicatif::{ProgressBar, ProgressStyle};
use std::env;

mod client;
mod config;

const DEFAULT_DAILY_TARGET: TimeDelta = TimeDelta::hours(8);

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(long)]
    ///Do not actually log work
    dry_run: bool,
    #[arg(long)]
    ///Fill timesheet for next week rather than current week
    next: bool,
    #[arg(long)]
    ///Submit timesheet for approval after adding work
    submit: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let config = config::load_config()?;
    let token = env::var("JIRA_TOKEN")?;

    let client = JtClient::new(&token, config.api_endpoint.clone(), args.dry_run);

    let now = chrono::Local::now();
    let week = if args.next {
        (now + TimeDelta::weeks(1)).iso_week()
    } else {
        now.iso_week()
    };
    let first_day =
        NaiveDate::from_isoywd_opt(now.year(), week.week(), chrono::Weekday::Mon).unwrap();
    let done_tasks_from = first_day - TimeDelta::days(1);

    let tasks = get_tasks(&client, done_tasks_from).await?;

    let target_per_day = config
        .daily_target_time_spent_seconds
        .map(|seconds| TimeDelta::seconds(seconds as i64))
        .unwrap_or(DEFAULT_DAILY_TARGET);
    let mut work = Vec::new();
    for day in first_day.iter_days().take(5) {
        let today = select_days_tasks(
            day,
            &tasks,
            target_per_day,
            config
                .default_time_spent_seconds
                .map(|seconds| TimeDelta::seconds(seconds as i64)),
        );
        let today = today
            .into_iter()
            .map(|(task, duration)| (day, task, duration));
        work.extend(today);
    }

    upload_worklogs(&client, &config, work).await?;

    if args.submit {
        submit(&client, &config, first_day).await?;
    }

    Ok(())
}

fn select_days_tasks(
    day: NaiveDate,
    tasks: &[Issue],
    target_per_day: TimeDelta,
    default_time_spent: Option<TimeDelta>,
) -> Vec<(&Issue, TimeDelta)> {
    let mut today = Vec::new();
    println!("{}", style(day.format("%A, %-d %B")).bold());
    while today
        .iter()
        .map(|(_, duration)| duration)
        .sum::<TimeDelta>()
        < target_per_day
    {
        let select = Select::new()
            .with_prompt("Select task")
            .items(tasks)
            .default(0)
            .interact()
            .unwrap();
        let selected = tasks.get(select).unwrap();
        let time_spent = if let Some(time) = default_time_spent {
            println!("Using default time spent");
            time
        } else {
            let input: u64 = Input::new()
                .with_prompt("How many minutes did you spend on this task?")
                .interact()
                .unwrap();
            TimeDelta::minutes(input as i64)
        };
        today.push((selected, time_spent));
    }
    today
}

async fn get_tasks(client: &JtClient, done_tasks_from: NaiveDate) -> Result<Vec<Issue>> {
    let spinner = ProgressBar::new_spinner().with_message(
        style("Retrieving assigned tasks from JIRA")
            .bold()
            .to_string(),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    let tasks = client.get_assigned_issues(done_tasks_from).await?;
    spinner.finish_and_clear();
    println!("{}", style("Assigned tasks retrieved").green());
    Ok(tasks)
}

async fn upload_worklogs(
    client: &JtClient,
    config: &Config,
    worklogs: Vec<(NaiveDate, &Issue, TimeDelta)>,
) -> Result<()> {
    let bar = ProgressBar::new(5)
        .with_style(ProgressStyle::with_template("{msg}\n{bar} {pos}/{len}").unwrap())
        .with_message(style("Logging work on Tempo").bold().to_string());
    for (day, log, time_spent) in worklogs {
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
            .create_worklog(
                &config.worker,
                day,
                &log.key,
                time_spent,
                resolved_attributes,
            )
            .await?;
        bar.inc(1);
    }
    bar.finish_and_clear();
    println!("{}", style("Work logged").green().bold());
    Ok(())
}

async fn submit(client: &JtClient, config: &Config, first_day: NaiveDate) -> Result<()> {
    if let Some(reviewer) = &config.reviewer {
        let spinner = ProgressBar::new_spinner()
            .with_message(style("Submitting timesheet").bold().to_string());
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        client
            .submit_timesheet(&config.worker, reviewer, first_day)
            .await?;
        spinner.finish_and_clear();
        println!("{}", style("Timesheet submitted").green().bold());
        Ok(())
    } else {
        bail!("No reviewer specified for submission")
    }
}
