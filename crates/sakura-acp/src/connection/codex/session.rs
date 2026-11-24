//! The thread of a conversation: opening it (a new one, or the stored one resumed and replayed), the models
//! the server offers, the person's prompts and the stop of a turn.

use super::{items, request_failed, ActiveTurn, Serve, APPROVAL_POLICY, SANDBOX};
use crate::attachment::Attachment;
use crate::connection::{AgentEvent, LoopEnd};
use crate::model::{Choice, Selector, SessionChoices};
use crate::thread::{Problem, ProblemKind, ThreadEvent};
use serde_json::{json, Value};
use tokio::time::Instant;

/// The id of the model selector: `session/set_config_option`'s `configId` of the window's model chip.
const MODEL_OPTION: &str = "model";
/// The ids of the other options the window sets: the effort, the fast tier, and the modes' names.
const EFFORT_OPTION: &str = "effort";
const SPEED_OPTION: &str = "service_tier";
const MODE_DEFAULT: &str = "default";
const MODE_PLAN: &str = "plan";
/// The id of the service tier that is "fast".
const FAST_TIER: &str = "priority";
/// Pages of `model/list` read.
const MAX_MODEL_PAGES: usize = 5;

impl Serve {
    pub(super) async fn open(
        &mut self,
        stored: Option<String>,
        allows_replacement: bool,
    ) -> Option<LoopEnd> {
        let opened = match stored {
            Some(stored) => self.resume(stored, allows_replacement).await,
            None => self.start().await,
        };
        match opened {
            Ok(event) => {
                let is_open = matches!(event, AgentEvent::SessionOpened { .. });
                self.emit(event).await;
                if is_open {
                    self.announce_commands().await;
                }
            }
            Err(problem) if problem.kind == ProblemKind::Died => return Some(LoopEnd::AgentClosed),
            Err(problem) => self.fail(problem).await,
        }
        None
    }

    /// What every thread of the master starts and resumes with. The person's own Codex configuration is
    /// read first, to switch off every server of it: a server the engine did not name is not the engine's to
    /// allow. When it cannot be read, no thread opens.
    async fn thread_params(&mut self) -> Result<serde_json::Map<String, Value>, Problem> {
        let limit = self.config.agent.deadlines.request;
        let folder = self.config.agent.workspace.display().to_string();
        let ask = json!({ "cwd": folder, "includeLayers": false });
        let read = self.call("config/read", ask, limit).await?;
        let inherited = crate::connection::CodexConfig::inherited_servers(&read);
        let mut params = serde_json::Map::new();
        params.insert("cwd".into(), json!(folder));
        params.insert("approvalPolicy".into(), json!(APPROVAL_POLICY));
        params.insert("sandbox".into(), json!(SANDBOX));
        params.insert(
            "developerInstructions".into(),
            json!(self.config.instructions),
        );
        params.insert("config".into(), self.config.thread_config(&inherited));
        if let Some(model) = &self.model {
            params.insert("model".into(), json!(model));
        }
        Ok(params)
    }

    async fn start(&mut self) -> Result<AgentEvent, Problem> {
        let limit = self.config.agent.deadlines.request;
        let mut params = self.thread_params().await?;
        params.insert("serviceName".into(), json!("sakura"));
        let answer = self
            .call("thread/start", Value::Object(params), limit)
            .await?;
        let session_id = self.take_thread(&answer)?;
        self.offer_models(&answer).await;
        Ok(AgentEvent::SessionOpened {
            session_id,
            is_loaded: false,
        })
    }

    async fn resume(
        &mut self,
        stored: String,
        allows_replacement: bool,
    ) -> Result<AgentEvent, Problem> {
        let limit = self.config.agent.deadlines.load;
        let mut params = self.thread_params().await?;
        params.insert("threadId".into(), json!(stored));
        self.emit_thread(ThreadEvent::ReplayStarted).await;
        match self
            .call("thread/resume", Value::Object(params), limit)
            .await
        {
            Ok(answer) => {
                self.thread_id = Some(stored.clone());
                self.replay(&answer).await;
                self.emit_thread(ThreadEvent::ReplayFinished).await;
                self.offer_models(&answer).await;
                Ok(AgentEvent::SessionOpened {
                    session_id: stored,
                    is_loaded: true,
                })
            }
            Err(problem) => {
                self.emit_thread(ThreadEvent::ReplayFinished).await;
                match problem.kind {
                    ProblemKind::SessionMissing if allows_replacement => self.start().await,
                    ProblemKind::SessionMissing => {
                        Ok(AgentEvent::SessionUnavailable { session_id: stored })
                    }
                    _ => Err(problem),
                }
            }
        }
    }

    /// The stored turns' items, in order, as what the conversation shows.
    async fn replay(&mut self, answer: &Value) {
        let turns = answer.get("thread").and_then(|thread| thread.get("turns"));
        let turns = turns.and_then(Value::as_array).into_iter().flatten();
        for item in turns
            .filter_map(|turn| turn.get("items")?.as_array())
            .flatten()
        {
            for update in items::replayed(item) {
                self.emit_thread(ThreadEvent::Update(update)).await;
            }
        }
    }

    /// The id of the thread a start answered with, kept as the session's.
    fn take_thread(&mut self, answer: &Value) -> Result<String, Problem> {
        let id = answer
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str);
        let id = id.ok_or_else(|| request_failed("thread/start", "the answer names no thread"))?;
        self.thread_id = Some(id.to_string());
        Ok(id.to_string())
    }

    /// Lists the models once and tells the window which one is in use: the person's choice, else the
    /// one the thread answered with.
    async fn offer_models(&mut self, answer: &Value) {
        if self.models.is_empty() {
            self.models = self.list_models().await;
        }
        self.reported_model = answer
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.emit_choices().await;
    }

    async fn list_models(&mut self) -> Vec<Choice> {
        let limit = self.config.agent.deadlines.request;
        let mut found = Vec::new();
        let mut cursor = Value::Null;
        for _ in 0..MAX_MODEL_PAGES {
            let params = json!({ "cursor": cursor, "includeHidden": true });
            let Ok(page) = self.call("model/list", params, limit).await else {
                break;
            };
            found.extend(
                page.get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(choice_of),
            );
            cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() {
                break;
            }
        }
        found
    }

    /// What the session lets the person pick: the models with the efforts each accepts, the effort of the
    /// chosen model, whether it runs fast where it can, and the collaboration mode.
    pub(super) fn choices(&self) -> SessionChoices {
        let current = self
            .model
            .clone()
            .or_else(|| self.reported_model.clone())
            .unwrap_or_default();
        let selected = self.models.iter().find(|choice| choice.id == current);
        SessionChoices {
            modes: Some(self.collaboration_selector()),
            models: (!self.models.is_empty()).then(|| Selector {
                name: "Model".to_string(),
                config_id: Some(MODEL_OPTION.to_string()),
                current,
                choices: self.models.clone(),
                is_toggle: false,
            }),
            efforts: selected.and_then(|model| self.effort_selector(model)),
            others: selected
                .and_then(|model| self.fast_selector(model))
                .into_iter()
                .collect(),
            ..SessionChoices::default()
        }
    }

    /// The efforts `model` accepts, with the one in force: the person's, when the model accepts it, else the
    /// model's own default.
    fn effort_selector(&self, model: &Choice) -> Option<Selector> {
        if model.efforts.is_empty() {
            return None;
        }
        let accepted = |id: &&String| model.efforts.iter().any(|effort| effort.id == **id);
        let current = self
            .effort
            .as_ref()
            .filter(accepted)
            .cloned()
            .or_else(|| model.default_effort.clone())
            .unwrap_or_default();
        Some(Selector {
            name: "Effort".to_string(),
            config_id: Some(EFFORT_OPTION.to_string()),
            current,
            choices: model.efforts.clone(),
            is_toggle: false,
        })
    }

    /// The fast tier of `model` as a switch, when the model offers one: off until the person turns it on.
    fn fast_selector(&self, model: &Choice) -> Option<Selector> {
        let tier = model.tiers.first()?;
        let choice = |id: &str, name: &str| Choice {
            id: id.to_string(),
            name: name.to_string(),
            ..Choice::default()
        };
        Some(Selector {
            name: tier.name.clone(),
            config_id: Some(SPEED_OPTION.to_string()),
            current: self.is_fast.to_string(),
            choices: vec![choice("false", "Off"), choice("true", "On")],
            is_toggle: true,
        })
    }

    /// Codex's two collaboration modes as the window's modes: it works on the task, or it plans.
    fn collaboration_selector(&self) -> Selector {
        let choice = |id: &str, name: &str| Choice {
            id: id.to_string(),
            name: name.to_string(),
            ..Choice::default()
        };
        Selector {
            name: "Mode".to_string(),
            config_id: None,
            current: self
                .collaboration
                .clone()
                .unwrap_or_else(|| MODE_DEFAULT.to_string()),
            choices: vec![choice(MODE_DEFAULT, "Default"), choice(MODE_PLAN, "Plan")],
            is_toggle: false,
        }
    }

    /// The person picked a model, an effort or the fast tier: it is the setting of this turn and the next ones.
    pub(super) async fn set_option(&mut self, config_id: String, value: String) {
        match config_id.as_str() {
            MODEL_OPTION if self.models.iter().any(|choice| choice.id == value) => {
                self.model = Some(value);
            }
            EFFORT_OPTION => self.effort = Some(value).filter(|value| !value.is_empty()),
            SPEED_OPTION => self.is_fast = value == "true",
            _ => return eprintln!("sakura: the codex master has no option {config_id} = {value}"),
        }
        self.emit_choices().await;
    }

    /// The person chose the collaboration mode `mode`: `plan` or `default`, kept for every turn after.
    pub(super) async fn set_collaboration(&mut self, mode: String) {
        if mode != MODE_DEFAULT && mode != MODE_PLAN {
            return eprintln!("sakura: the codex master has no mode {mode}");
        }
        self.collaboration = Some(mode);
        self.emit_choices().await;
    }

    /// What every turn says of the person's choices: the model, the effort, the service tier and the
    /// collaboration mode, each only when it was chosen, so that a turn on Codex's own defaults names nothing.
    fn turn_settings(&self) -> serde_json::Map<String, Value> {
        let mut settings = serde_json::Map::new();
        if let Some(model) = &self.model {
            settings.insert("model".into(), json!(model));
        }
        if let Some(effort) = &self.effort {
            settings.insert("effort".into(), json!(effort));
        }
        if self.is_fast {
            settings.insert("serviceTier".into(), json!(FAST_TIER));
        }
        if let Some(mode) = &self.collaboration {
            let current = self.model.clone().or_else(|| self.reported_model.clone());
            if let Some(model) = current {
                let effort = self.effort.clone();
                let mode = json!({ "mode": mode, "settings": { "model": model, "reasoning_effort": effort, "developer_instructions": null } });
                settings.insert("collaborationMode".into(), mode);
            }
        }
        settings
    }

    /// Starts a turn with the person's words and files.
    pub(super) async fn prompt(&mut self, text: String, attachments: Vec<Attachment>) {
        if self.turn.is_some() {
            return eprintln!("sakura: a prompt arrived while a turn was running and was not sent");
        }
        if attachments.is_empty() && self.try_slash(&text).await {
            return;
        }
        self.start_turn(input_of(&text, &attachments)).await;
    }

    /// Starts a turn on the thread with `input`, in the settings the person chose.
    pub(super) async fn start_turn(&mut self, input: Vec<Value>) {
        let Some(thread) = self.thread_id.clone() else {
            return self
                .fail(request_failed("turn/start", "no thread is open"))
                .await;
        };
        if self.turn.is_some() {
            return eprintln!("sakura: a prompt arrived while a turn was running and was not sent");
        }
        let mut params = json!({ "threadId": thread, "input": input });
        for (name, value) in self.turn_settings() {
            params[name] = value;
        }
        self.last_heard = Instant::now();
        self.turn = Some(ActiveTurn {
            id: None,
            cancel_limit: None,
        });
        let limit = self.config.agent.deadlines.request;
        match self.call("turn/start", params, limit).await {
            Ok(answer) => self.turn_answered(&answer),
            Err(problem) => {
                self.turn = None;
                self.fail(problem).await;
            }
        }
    }

    /// The server's id of the turn that just started, unless it already ended.
    pub(super) fn turn_answered(&mut self, answer: &Value) {
        let id = answer
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str);
        if let (Some(turn), Some(id)) = (self.turn.as_mut(), id) {
            turn.id.get_or_insert_with(|| id.to_string());
        }
    }

    /// Tells the server to stop the turn, answers every waiting approval "cancel" and starts counting the
    /// time the server has to end the turn.
    pub(super) async fn interrupt(&mut self) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        turn.cancel_limit = Some(Instant::now() + self.config.agent.deadlines.cancel);
        let ids = (self.thread_id.clone(), turn.id.clone());
        if let (Some(thread), Some(turn)) = ids {
            self.notify_server(
                "turn/interrupt",
                json!({ "threadId": thread, "turnId": turn }),
            )
            .await;
        }
        self.cancel_waiting().await;
    }
}

fn choice_of(model: &Value) -> Option<Choice> {
    let id = model.get("model")?.as_str()?;
    let name = model
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or(id);
    let description = model
        .get("description")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    let listed = |key: &str| {
        model
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    };
    let efforts = listed("supportedReasoningEfforts").filter_map(|effort| {
        let id = effort.get("reasoningEffort")?.as_str()?;
        let description = effort.get("description").and_then(Value::as_str);
        Some(Choice {
            id: id.to_string(),
            name: id.to_string(),
            description: description
                .filter(|text| !text.is_empty())
                .map(str::to_string),
            ..Choice::default()
        })
    });
    let tiers = listed("serviceTiers").filter_map(|tier| {
        let id = tier.get("id")?.as_str()?;
        let name = tier.get("name").and_then(Value::as_str).unwrap_or(id);
        Some(Choice {
            id: id.to_string(),
            name: name.to_string(),
            ..Choice::default()
        })
    });
    Some(Choice {
        id: id.to_string(),
        name: name.to_string(),
        description: description.map(str::to_string),
        efforts: efforts.collect(),
        default_effort: model
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .map(str::to_string),
        tiers: tiers.collect(),
        is_hidden: model
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The turn's input: the person's words, then each file. An image goes as a data address and a text file
/// as text after the words; a PDF cannot be read by Codex, and the prompt says so instead of dropping it
/// without a word.
fn input_of(text: &str, attachments: &[Attachment]) -> Vec<Value> {
    let words = |text: String| json!({ "type": "text", "text": text, "text_elements": [] });
    let mut input = vec![words(text.to_string())];
    for attachment in attachments {
        let block = attachment.block();
        match block.get("type").and_then(Value::as_str) {
            Some("image") => input.push(image_input(&block)),
            _ => input.push(words(file_note(attachment.name(), &block))),
        }
    }
    input
}

fn image_input(block: &Value) -> Value {
    let mime = block
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    let data = block
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({ "type": "image", "url": format!("data:{mime};base64,{data}") })
}

fn file_note(name: &str, block: &Value) -> String {
    let resource = block.get("resource");
    match resource
        .and_then(|resource| resource.get("text"))
        .and_then(Value::as_str)
    {
        Some(text) => format!("The file {name} the person attached:\n\n{text}"),
        None => format!("The person attached {name}, which Codex cannot read: it was not sent."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_come_first_then_an_image_as_a_data_address_and_a_text_file_as_text() {
        let image =
            Attachment::from_bytes("shot.png", "image/png", vec![1, 2, 3]).expect("an image");
        let note =
            Attachment::from_bytes("notes.md", "text/plain", b"# Notes".to_vec()).expect("text");
        let input = input_of("look", &[image, note]);
        assert_eq!(input[0]["text"], "look");
        assert_eq!(input[1]["type"], "image");
        assert_eq!(input[1]["url"], "data:image/png;base64,AQID");
        assert!(input[2]["text"]
            .as_str()
            .is_some_and(|text| text.contains("notes.md") && text.contains("# Notes")));
    }

    #[test]
    fn a_pdf_is_named_as_not_sent_instead_of_dropped_silently() {
        let pdf = Attachment::from_bytes("plan.pdf", "application/pdf", vec![9]).expect("a pdf");
        let input = input_of("read", &[pdf]);
        let note = input[1]["text"].as_str().unwrap_or_default();
        assert!(
            note.contains("plan.pdf") && note.contains("not sent"),
            "{note}"
        );
    }

    #[test]
    fn a_model_of_the_list_is_a_choice_named_by_its_display_name() {
        let model =
            json!({ "id": "x", "model": "gpt-x", "displayName": "GPT X", "description": "fast" });
        let choice = choice_of(&model).expect("a choice");
        assert_eq!(
            (choice.id.as_str(), choice.name.as_str()),
            ("gpt-x", "GPT X")
        );
        assert_eq!(choice.description.as_deref(), Some("fast"));
        assert_eq!(choice_of(&json!({})), None);
    }
}
