//! The commands Codex's own terminal has that its app-server has a request for: each is announced in the slash
//! menu with the words the terminal gives it, and typing it runs that request instead of being sent as a
//! message. What comes back is said in the conversation as the agent's words.
//!
//! The terminal's other commands have no request (`/init`, `/diff`, `/mention`, `/copy`, `/export`, `/theme`,
//! `/statusline`, `/title`, `/resume`, `/side`, `/ps`, `/cwd`, `/raw`, `/personality`, `/permissions`) or are the
//! window's own (`/model`, `/plan`, `/clear`, `/new`, `/stop`).

use super::{ActiveTurn, Serve};
use crate::connection::AgentEvent;
use crate::model::{SlashCommand, StopReason, Update};
use crate::thread::ThreadEvent;
use serde_json::{json, Value};

/// The commands, with Codex's own description of each (read from the terminal's own list).
const COMMANDS: [(&str, &str, Option<&str>); 13] = [
    ("compact", "summarize the current conversation now", None),
    (
        "review",
        "review my current changes and find issues",
        Some("[base branch]"),
    ),
    ("fork", "fork the current chat", None),
    ("rename", "rename the current thread", Some("<name>")),
    (
        "skills",
        "use skills to improve how Codex performs specific tasks",
        None,
    ),
    (
        "goal",
        "set or view the goal for a long-running task",
        Some("[objective | clear]"),
    ),
    ("mcp", "list MCP tools", None),
    (
        "status",
        "show current session configuration and token usage",
        None,
    ),
    ("apps", "manage apps", None),
    ("plugins", "browse plugins", None),
    ("hooks", "view and manage lifecycle hooks", None),
    ("experimental", "toggle experimental features", None),
    (
        "usage",
        "view account usage or use a usage limit reset",
        None,
    ),
];

/// A skill the thread offers: its name, what it is for and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Skill {
    pub name: String,
    pub description: String,
    pub path: String,
}

impl Serve {
    /// Tells the window which commands there are: the terminal's own, then each skill by its name.
    pub(super) async fn announce_commands(&mut self) {
        self.skills = self.list_skills().await;
        let mut commands: Vec<SlashCommand> = COMMANDS
            .iter()
            .map(|(name, words, hint)| SlashCommand {
                name: name.to_string(),
                description: words.to_string(),
                hint: hint.map(str::to_string),
            })
            .collect();
        commands.extend(self.skills.iter().map(|skill| SlashCommand {
            name: skill.name.clone(),
            description: skill.description.clone(),
            hint: Some("[words]".to_string()),
        }));
        self.emit(AgentEvent::Commands(commands)).await;
    }

    async fn list_skills(&mut self) -> Vec<Skill> {
        let limit = self.config.agent.deadlines.request;
        let folder = self.config.agent.workspace.display().to_string();
        let ask = json!({ "cwds": [folder], "forceReload": false });
        let Ok(answer) = self.call("skills/list", ask, limit).await else {
            return Vec::new();
        };
        let lists = answer
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        let skills = lists.flat_map(|list| {
            list.get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        });
        skills
            .filter(|skill| {
                skill
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            })
            .filter_map(|skill| {
                let text = |key: &str| skill.get(key).and_then(Value::as_str).map(str::to_string);
                Some(Skill {
                    name: text("name")?,
                    description: text("description").unwrap_or_default(),
                    path: text("path")?,
                })
            })
            .collect()
    }

    /// Runs `text` when it is one of the commands: true when it was taken (and nothing is sent as a message).
    pub(super) async fn try_slash(&mut self, text: &str) -> bool {
        let Some(typed) = text.trim().strip_prefix('/') else {
            return false;
        };
        let (name, words) = typed.split_once(char::is_whitespace).unwrap_or((typed, ""));
        let words = words.trim();
        if let Some(skill) = self.skills.iter().find(|skill| skill.name == name).cloned() {
            self.run_skill(&skill, words).await;
            return true;
        }
        if !COMMANDS.iter().any(|(known, _, _)| *known == name) {
            return false;
        }
        let Some(thread) = self.thread_id.clone() else {
            return false;
        };
        match name {
            "compact" => {
                self.side_turn("thread/compact/start", json!({ "threadId": thread }))
                    .await
            }
            "review" => {
                let target = match words.is_empty() {
                    true => json!({ "type": "uncommittedChanges" }),
                    false => json!({ "type": "baseBranch", "branch": words }),
                };
                self.side_turn(
                    "review/start",
                    json!({ "threadId": thread, "target": target }),
                )
                .await
            }
            _ => {
                let said = self.ask_for_words(name, words, &thread).await;
                self.say_and_end(said).await
            }
        }
        true
    }

    /// A request whose answer is a turn of its own (a compaction, a review): it runs like any turn.
    async fn side_turn(&mut self, method: &'static str, params: Value) {
        let limit = self.config.agent.deadlines.request;
        self.turn = Some(ActiveTurn {
            id: None,
            cancel_limit: None,
        });
        match self.call(method, params, limit).await {
            Ok(answer) => self.turn_answered(&answer),
            Err(problem) => {
                self.turn = None;
                self.fail(problem).await;
            }
        }
    }

    /// The commands that answer in words: what the request said, as the agent's message.
    async fn ask_for_words(&mut self, name: &str, words: &str, thread: &str) -> String {
        let limit = self.config.agent.deadlines.request;
        let result = match name {
            "fork" => self
                .call("thread/fork", json!({ "threadId": thread }), limit)
                .await
                .map(|answer| {
                    let id = answer
                        .pointer("/thread/id")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    format!("Forked this chat into the thread {id}.")
                }),
            "rename" if words.is_empty() => Ok("Say the new name: /rename <name>.".to_string()),
            "rename" => self
                .call(
                    "thread/name/set",
                    json!({ "threadId": thread, "name": words }),
                    limit,
                )
                .await
                .map(|_| format!("Renamed the thread to {words}.")),
            "skills" => Ok(skills_words(&self.skills)),
            "goal" => self.goal(words, thread).await,
            "mcp" => self
                .call("mcpServerStatus/list", json!({}), limit)
                .await
                .map(|answer| mcp_words(&answer)),
            "status" => self.status_words().await,
            "usage" => self
                .call("account/rateLimits/read", json!({}), limit)
                .await
                .map(|answer| limits_words(&answer)),
            "apps" => self.count_words("app/list", "apps").await,
            "plugins" => self.count_words("plugin/list", "plugins").await,
            "hooks" => self.count_words("hooks/list", "hooks").await,
            _ => {
                self.count_words("experimentalFeature/list", "experimental features")
                    .await
            }
        };
        result.unwrap_or_else(|problem| problem.detail)
    }

    async fn goal(&mut self, words: &str, thread: &str) -> Result<String, crate::thread::Problem> {
        let limit = self.config.agent.deadlines.request;
        match words {
            "" => {
                let answer = self
                    .call("thread/goal/get", json!({ "threadId": thread }), limit)
                    .await?;
                Ok(
                    match answer.pointer("/goal/objective").and_then(Value::as_str) {
                        Some(goal) => format!("The goal: {goal}."),
                        None => "No goal is set. Set one with /goal <objective>.".to_string(),
                    },
                )
            }
            "clear" => {
                self.call("thread/goal/clear", json!({ "threadId": thread }), limit)
                    .await?;
                Ok("The goal is cleared.".to_string())
            }
            objective => {
                let params = json!({ "threadId": thread, "objective": objective });
                self.call("thread/goal/set", params, limit).await?;
                Ok(format!("The goal is set: {objective}."))
            }
        }
    }

    async fn status_words(&mut self) -> Result<String, crate::thread::Problem> {
        let limit = self.config.agent.deadlines.request;
        let answer = self
            .call("account/rateLimits/read", json!({}), limit)
            .await?;
        let model = self.model.clone().or_else(|| self.reported_model.clone());
        let model = model.unwrap_or_else(|| "Codex's own default".to_string());
        Ok(format!("Model: {model}. {}", limits_words(&answer)))
    }

    /// A list the server keeps, said as how many there are.
    async fn count_words(
        &mut self,
        method: &'static str,
        what: &str,
    ) -> Result<String, crate::thread::Problem> {
        let limit = self.config.agent.deadlines.request;
        let answer = self.call(method, json!({}), limit).await?;
        let count = answer
            .get("data")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        Ok(format!("{count} {what}."))
    }

    /// Runs a skill: a turn whose message names the skill, with the person's words after it.
    async fn run_skill(&mut self, skill: &Skill, words: &str) {
        let mut input = vec![json!({ "type": "skill", "name": skill.name, "path": skill.path })];
        if !words.is_empty() {
            input.push(json!({ "type": "text", "text": words, "text_elements": [] }));
        }
        self.start_turn(input).await;
    }

    /// Says `words` as the agent's and ends the turn.
    async fn say_and_end(&mut self, words: String) {
        self.emit_thread(ThreadEvent::Update(Update::AgentText(words)))
            .await;
        self.emit_thread(ThreadEvent::TurnEnded(StopReason::EndTurn))
            .await;
    }
}

fn skills_words(skills: &[Skill]) -> String {
    match skills.is_empty() {
        true => "No skills.".to_string(),
        false => skills
            .iter()
            .map(|skill| format!("{}: {}", skill.name, skill.description))
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

fn mcp_words(answer: &Value) -> String {
    let servers = answer
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let lines: Vec<String> = servers
        .map(|server| {
            let name = server.get("name").and_then(Value::as_str).unwrap_or("?");
            let tools = server
                .get("tools")
                .and_then(Value::as_object)
                .map_or(0, |tools| tools.len());
            format!("{name}: {tools} tools")
        })
        .collect();
    match lines.is_empty() {
        true => "No MCP servers.".to_string(),
        false => lines.join("\n\n"),
    }
}

fn limits_words(answer: &Value) -> String {
    let used = answer
        .pointer("/rateLimits/primary/usedPercent")
        .and_then(Value::as_f64);
    match used {
        Some(used) => format!("{used:.0}% of the current window is used."),
        None => "The server reports no usage.".to_string(),
    }
}
