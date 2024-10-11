# jt

Fill out your weekly Tempo timesheet some your terminal.

## Installation

Presently only JIRA server is supported.

TBC

## Usage

You personal JIRA token is read as an environment variable named `JIRA_TOKEN`.
To create such a token follow the instructions
[here](https://confluence.atlassian.com/enterprise/using-personal-access-tokens-1026032365.html).

Once the token is in place for an initial configuration it is recommended to run
`init`:

```sh
jt init
```

This will run a short wizard that will populate an initial configuration file
for you, calling your JIRA instance where necessary to populate values.

### Configuration options

Below is an example configuration file.

```toml
api_endpoint = "https://jira.mycompany.com/"
worker = "JIRAUSER12345"
reviewer = "JIRAUSER6789"
default_time_spent_minutes = 480 # 8 hours
daily_target_time_spent_minutes = 480 # 8 hours

[[static_tasks]]
key = "TEMPO-1"
description = "Time off"
attributes = [
  { key = "my_attr_key", name = "Some Attribute", work_attribute_id = 1, value = "SomeValue" },
]

[[static_attributes]]
key = "my_attr_key"
name = "Some Attribute"
work_attribute_id = 1
value = "SomeValue"

[[dynamic_attributes]]
key = "my_dynamic_attr_key"
name = "Another Attribute"
work_attribute_id = 123
value = "/customfield_12345/key"

```

#### Static tasks

By default jt will query your currently assigned tasks to construct the list of
tasks to choose for each day. If there are tasks outside of this list that you
need to use, for example a task representing holidays, you can manually define
it as a "static task" that will always be included in the tasks list.

#### Attributes

Attributes are metadata fields that Tempo associates with each work log. Where
the values of these fields can be hardcoded static attributes can be used, both
for specific static tasks as well as globally for all tasks retrieved via query.

Sometimes this is not sufficient and the value of one of these fields must be
determined dynamically. For this scenario a limited form of dynamic attribute is
supported, where the `value`, rather than being a hardcoded string, is instead a
[JSON pointer](https://www.rfc-editor.org/rfc/rfc6901) that will be resolved
against the fields of the selected task.

In order to figure out the combination of static and dynamic attributes you need
for your particular JIRA/Tempo setup it is recommended to use your browsers
network tools to understand which attributes are typically populated when
filling out the timesheet using the web interface.

### CLI options

```
Usage: jt <COMMAND>

Commands:
  fill  Fill a timesheet
  init  Generate a configuration file
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
