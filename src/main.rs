use anyhow::{bail, Result};
use chrono::{Datelike, NaiveDate, TimeDelta};
use clap::{Parser, Subcommand};
use client::{Issue, JtClient};
use config::{Config, WorkAttribute};
use console::style;
use dialoguer::{Confirm, Input, Select};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Url;
use std::env;

mod client;
mod config;

const DEFAULT_DAILY_TARGET: TimeDelta = TimeDelta::hours(8);

#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    ///Fill a timesheet
    Fill {
        #[arg(long)]
        ///Do not actually log work
        dry_run: bool,
        #[arg(long)]
        ///Fill timesheet for next week rather than current week
        next: bool,
        #[arg(long)]
        ///Submit timesheet for approval after adding work
        submit: bool,
    },
    ///Generate a configuration file
    Init,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let token = env::var("JIRA_TOKEN")?;

    match args.command {
        Commands::Fill {
            dry_run,
            next,
            submit,
        } => fill(token, dry_run, next, submit).await,
        Commands::Init => init(token).await,
    }
}

async fn init(token: String) -> Result<()> {
    let endpoint: Url = Input::new()
        .with_prompt("JIRA instance URL (eg \"https://jira.yourcompany.com\")")
        .interact()
        .unwrap();
    let client = JtClient::new(&token, endpoint.clone(), true);
    let spinner = ProgressBar::new_spinner().with_message("Validating instance URL");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    client.health_check().await?;
    spinner.finish_and_clear();
    println!("{}", style("Instance URL validated").green());

    let username: String = Input::new()
        .with_prompt("Your JIRA username (eg \"jsmith\")")
        .interact()
        .unwrap();
    let spinner = ProgressBar::new_spinner().with_message("Retrieving user key");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    let user_key = client.get_user_key(&username).await?;
    spinner.finish_and_clear();
    println!("{}", style("User key retrieved").green());

    let daily_time_target: u64 = Input::new()
        .with_prompt("Your daily target for time spent on tasks (in minutes, default is equivalent to 8 hours)")
        .default(480)
        .interact()
        .unwrap();

    let specify_reviewer = Confirm::new()
        .with_prompt("Specify reviewer? (enables submission)")
        .default(true)
        .interact()
        .unwrap();
    let reviewer = if specify_reviewer {
        let reviewer_username: String = Input::new()
            .with_prompt("Your reviewer's JIRA username (eg \"jsmith\")")
            .interact()
            .unwrap();
        let spinner = ProgressBar::new_spinner().with_message("Retrieving reviewer key");
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        let reviewer_key = client.get_user_key(&reviewer_username).await?;
        spinner.finish_and_clear();
        println!("{}", style("Reviewer key retrieved").green());
        Some(reviewer_key)
    } else {
        None
    };

    let config = Config {
        api_endpoint: endpoint,
        worker: user_key,
        reviewer,
        daily_target_time_spent_seconds: Some(daily_time_target * 60),
        default_time_spent_seconds: None,
        static_tasks: Vec::new(),
        static_attributes: Vec::new(),
        dynamic_attributes: Vec::new(),
    };
    config::write_config(config)?;
    println!(
        "\n{}\n",
        style(format!(
            "Configuration written to {}",
            config::config_file_location().to_str().unwrap()
        ))
        .green()
        .bold()
    );
    println!("Further options and customisations are available by manually editing the configuration file.");
    Ok(())
}

async fn fill(token: String, dry_run: bool, next: bool, auto_submit: bool) -> Result<()> {
    let config = config::load_config()?;
    let client = JtClient::new(&token, config.api_endpoint.clone(), dry_run);

    let now = chrono::Local::now();
    let week = if next {
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

    if auto_submit {
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
    let bar = ProgressBar::new(worklogs.len() as u64)
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
