use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, NaiveDate, TimeDelta};
use clap::{Parser, Subcommand};
use client::{Issue, JtClient};
use config::{Config, StaticTask, WorkAttribute};
use console::style;
use dialoguer::{Confirm, Input, Select};
use indicatif::{ProgressBar, ProgressStyle};
use rand::{seq::SliceRandom, thread_rng};
use reqwest::Url;
use std::{env, fmt::Display};

mod client;
mod config;

const DEFAULT_DAILY_TARGET: TimeDelta = TimeDelta::hours(8);

enum Task {
    Static(StaticTask),
    FromQuery(Issue),
}

impl Task {
    fn key(&self) -> String {
        match self {
            Task::Static(s) => s.key.clone(),
            Task::FromQuery(f) => f.key.clone(),
        }
    }
}

impl Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Static(s) => write!(f, "{} - {}", s.key, s.description),
            Task::FromQuery(q) => write!(f, "{}", q),
        }
    }
}

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
        #[arg(long)]
        ///Select task at random rather than prompting
        random: bool,
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
            random,
        } => fill(token, dry_run, next, submit, random).await,
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
        daily_target_time_spent_minutes: Some(daily_time_target),
        default_time_spent_minutes: None,
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

async fn fill(
    token: String,
    dry_run: bool,
    next: bool,
    auto_submit: bool,
    random: bool,
) -> Result<()> {
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

    let issues = get_tasks(&client, done_tasks_from).await?;
    let mut tasks: Vec<Task> = issues.into_iter().map(Task::FromQuery).collect();
    tasks.extend(config.static_tasks.into_iter().map(Task::Static));

    let target_per_day = config
        .daily_target_time_spent_minutes
        .map(|minutes| TimeDelta::minutes(minutes as i64))
        .unwrap_or(DEFAULT_DAILY_TARGET);
    let mut work = Vec::new();
    for day in first_day.iter_days().take(5) {
        let today = select_days_tasks(
            day,
            &tasks,
            target_per_day,
            config
                .default_time_spent_minutes
                .map(|minutes| TimeDelta::minutes(minutes as i64)),
            random,
        )?;
        let today = today
            .into_iter()
            .map(|(task, duration)| (day, task, duration));
        work.extend(today);
    }

    upload_worklogs(
        &client,
        config.dynamic_attributes,
        config.static_attributes,
        &config.worker,
        work,
    )
    .await?;

    if auto_submit {
        submit(&client, config.reviewer, &config.worker, first_day).await?;
    }

    Ok(())
}

fn select_days_tasks(
    day: NaiveDate,
    tasks: &[Task],
    target_per_day: TimeDelta,
    default_time_spent: Option<TimeDelta>,
    random: bool,
) -> Result<Vec<(&Task, TimeDelta)>> {
    let mut today = Vec::new();
    println!("{}", style(day.format("%A, %-d %B")).bold());
    while today
        .iter()
        .map(|(_, duration)| duration)
        .sum::<TimeDelta>()
        < target_per_day
    {
        let (selected, time_spent) = if random {
            let time_spent = default_time_spent.ok_or(anyhow!(""))?;
            let selected = tasks.choose(&mut thread_rng()).unwrap();
            println!(
                "selected {} at random, assigning default time spent",
                selected.key()
            );
            (selected, time_spent)
        } else {
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
            (selected, time_spent)
        };
        today.push((selected, time_spent));
    }
    Ok(today)
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
    dynamic_attributes: Vec<WorkAttribute>,
    static_attributes: Vec<WorkAttribute>,
    worker: &str,
    worklogs: Vec<(NaiveDate, &Task, TimeDelta)>,
) -> Result<()> {
    let bar = ProgressBar::new(worklogs.len() as u64)
        .with_style(ProgressStyle::with_template("{msg}\n{bar} {pos}/{len}").unwrap())
        .with_message(style("Logging work on Tempo").bold().to_string());
    for (day, log, time_spent) in worklogs {
        let attributes = match log {
            Task::Static(task) => task.attributes.clone(),
            Task::FromQuery(issue) => {
                resolve_attributes(issue, &static_attributes, &dynamic_attributes)?
            }
        };
        client
            .create_worklog(worker, day, &log.key(), time_spent, attributes)
            .await?;
        bar.inc(1);
    }
    bar.finish_and_clear();
    println!("{}", style("Work logged").green().bold());
    Ok(())
}

fn resolve_attributes(
    issue: &Issue,
    static_attributes: &[WorkAttribute],
    dynamic_attributes: &[WorkAttribute],
) -> Result<Vec<WorkAttribute>> {
    let mut resolved = dynamic_attributes
        .iter()
        .map(|attr| {
            let pointable = serde_json::to_value(&issue.fields).unwrap();
            let pointed = pointable
                .pointer(&attr.value)
                .context("Unable to resolve JSON pointer")?
                .as_str()
                .context("JSON pointer does not point to string value")?;
            let mut evaluated = attr.clone();
            evaluated.value = pointed.to_owned();
            Ok(evaluated)
        })
        .collect::<Result<Vec<WorkAttribute>>>()?;
    resolved.extend_from_slice(static_attributes);
    Ok(resolved)
}

async fn submit(
    client: &JtClient,
    reviewer: Option<String>,
    worker: &str,
    first_day: NaiveDate,
) -> Result<()> {
    if let Some(reviewer) = reviewer {
        let spinner = ProgressBar::new_spinner()
            .with_message(style("Submitting timesheet").bold().to_string());
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        client
            .submit_timesheet(worker, &reviewer, first_day)
            .await?;
        spinner.finish_and_clear();
        println!("{}", style("Timesheet submitted").green().bold());
        Ok(())
    } else {
        bail!("No reviewer specified for submission")
    }
}
