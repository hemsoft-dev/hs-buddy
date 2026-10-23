//! The launch form. Every field maps to one `Launch` input; `buddy_core` validates them.
use std::collections::BTreeMap;
use std::path::PathBuf;

use buddy_core::ralph::launch::{Launch, MAX_CYCLES, MAX_ITERATIONS, ReviewAgent, Script};
use buddy_core::ralph::scripts::{Catalog, Template};
use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Window};

use super::RalphView;

/// One option of a picker; an empty value means the catalog default.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    pub value: String,
    pub title: SharedString,
}

impl SearchableListItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &String {
        &self.value
    }
}

fn choice(value: &str, title: String) -> Choice {
    Choice {
        value: value.to_owned(),
        title: title.into(),
    }
}

/// Models the provider can run, with their aliases and tiers, after a default entry.
pub(super) fn model_choices(catalog: &Catalog, provider: &str) -> Vec<Choice> {
    // A tier or alias names the model it stands for, as the alias entries below do.
    let default = catalog.effective_model(provider, "");
    let label = match catalog.model(default) {
        Some(model) if model.key != default => format!("Default ({default} → {})", model.key),
        _ => format!("Default ({default})"),
    };
    let mut choices = vec![choice("", label)];
    choices.extend(
        catalog
            .models
            .iter()
            .filter(|model| catalog.supports(provider, &model.key))
            .map(|model| {
                let effort = model
                    .effort
                    .as_deref()
                    .map(|effort| format!(" · {effort}"))
                    .unwrap_or_default();
                choice(&model.key, format!("{}{effort}", model.label))
            }),
    );
    choices.extend(
        catalog
            .aliases
            .iter()
            .filter(|(alias, _)| catalog.supports(provider, alias))
            .map(|(alias, target)| choice(alias, format!("{alias} → {target}"))),
    );
    choices
}

pub(super) fn provider_choices(catalog: &Catalog) -> Vec<Choice> {
    let mut choices = vec![choice(
        "",
        format!("Default ({})", catalog.default_provider),
    )];
    choices.extend(catalog.providers.iter().map(|provider| {
        choice(
            &provider.key,
            format!("{} — {}", provider.key, provider.description),
        )
    }));
    choices
}

pub(super) fn agent_choices(catalog: &Catalog) -> Vec<Choice> {
    let mut choices = vec![choice(
        "",
        format!("Default ({})", catalog.default_dev_agent),
    )];
    choices.extend(
        catalog
            .dev_agents
            .iter()
            .map(|agent| choice(&agent.key, format!("{} — {}", agent.key, agent.description))),
    );
    choices
}

/// `provider:model` for every model each provider can run.
pub(super) fn review_model_choices(catalog: &Catalog) -> Vec<Choice> {
    let mut choices = vec![choice("", "Default model".into())];
    for provider in &catalog.providers {
        for model in &catalog.models {
            if catalog.supports(&provider.key, &model.key) {
                choices.push(choice(
                    &format!("{}:{}", provider.key, model.key),
                    format!("{} · {}", provider.key, model.label),
                ));
            }
        }
    }
    choices
}

/// A whole number within `range`, or `message`.
pub(super) fn whole(
    text: &str,
    range: std::ops::RangeInclusive<u32>,
    message: &str,
) -> Result<u32, String> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| range.contains(value))
        .ok_or_else(|| message.to_owned())
}

pub(super) fn script_name(script: &Script, templates: &[Template]) -> String {
    match script {
        Script::Ralph => "Ralph Loop".into(),
        Script::Pr => "Ralph PR".into(),
        Script::Issues => "Ralph Issues".into(),
        Script::Template(file) => templates
            .iter()
            .find(|template| &template.file == file)
            .map(|template| template.name.clone())
            .unwrap_or_else(|| file.clone()),
    }
}

type Picker = Entity<SelectState<Vec<Choice>>>;

pub(super) struct Form {
    pub script: Script,
    pub fields: BTreeMap<&'static str, Entity<InputState>>,
    pub prompt: Entity<TextareaState>,
    pub provider: Picker,
    pub model: Picker,
    pub agent: Picker,
    /// Every review role with its description, in catalog order.
    pub reviews: Vec<(String, String)>,
    /// Selected review roles and their model pickers.
    pub selected: BTreeMap<String, Picker>,
    review_models: Vec<Choice>,
    pub dry_run: bool,
    pub merge: bool,
    /// The GitHub account a handoff came from; the loop works as it.
    pub account: Option<String>,
}

fn picker(
    items: Vec<Choice>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<RalphView>,
) -> Picker {
    let selected = items
        .iter()
        .position(|item| item.value == value)
        .or(Some(0))
        .map(IndexPath::new);
    let picker = cx.new(|cx| SelectState::new(items, selected, window, cx).searchable(true));
    cx.subscribe(&picker, |_, _, _: &SelectEvent<Vec<Choice>>, cx| {
        cx.notify()
    })
    .detach();
    picker
}

/// The repository field as typed or chosen. Leading and trailing spaces can belong to a
/// folder's name on macOS and Linux, so only a blank field counts as empty.
pub(super) fn repository_path(value: &str) -> PathBuf {
    if value.trim().is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(value)
    }
}

fn selected(picker: &Picker, cx: &gpui_kit::App) -> String {
    picker
        .read(cx)
        .selected_value()
        .cloned()
        .unwrap_or_default()
}

impl Form {
    /// A form for `script`, filled from `previous` (a retried run) or the defaults.
    pub fn new(
        script: Script,
        previous: Option<&Launch>,
        repository: String,
        catalog: &Catalog,
        templates: &[Template],
        window: &mut Window,
        cx: &mut Context<RalphView>,
    ) -> Self {
        let base = previous.cloned().unwrap_or_else(|| Launch {
            prompt: match &script {
                Script::Template(file) => templates
                    .iter()
                    .find(|template| &template.file == file)
                    .and_then(|template| template.default_prompt.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            },
            ..Launch::default()
        });
        let mut fields = BTreeMap::new();
        for (name, value) in [
            (
                "repository",
                if previous.is_some() {
                    base.repository.display().to_string()
                } else {
                    repository
                },
            ),
            ("iterations", base.iterations.to_string()),
            ("cycles", base.cycles.to_string()),
            ("branch", base.branch.clone()),
            (
                "pr",
                base.pr_number
                    .map(|number| number.to_string())
                    .unwrap_or_default(),
            ),
            ("labels", base.labels.clone()),
        ] {
            let input = cx.new(|cx| {
                let input = InputState::new(window, cx);
                match name {
                    "repository" => input.placeholder(if cfg!(windows) {
                        "D:\\github\\org\\repo"
                    } else {
                        "/home/me/github/org/repo"
                    }),
                    "branch" => input.placeholder("Leave empty to create one"),
                    "pr" => input.placeholder("e.g. 42"),
                    "labels" => input.placeholder("e.g. tech-debt,automated"),
                    _ => input,
                }
            });
            input.update(cx, |input, cx| input.set_value(value, window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
            fields.insert(name, input);
        }
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(match script {
                Script::Ralph => "What the loop should work on",
                Script::Issues => "What to scan for, e.g. security vulnerabilities",
                _ => "Optional: the script has its own default prompt",
            })
        });
        prompt.update(cx, |input, cx| {
            input.set_value(base.prompt.clone(), window, cx)
        });
        cx.subscribe(&prompt, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let provider = picker(provider_choices(catalog), &base.provider, window, cx);
        let model = picker(
            model_choices(catalog, &base.provider),
            &base.model,
            window,
            cx,
        );
        // A provider change keeps the model only when the new provider can run it.
        let models = catalog.clone();
        cx.subscribe_in(
            &provider,
            window,
            move |this, _, event: &SelectEvent<Vec<Choice>>, window, cx| {
                let (SelectEvent::Confirm(value), Some(form)) = (event, &mut this.form) else {
                    return;
                };
                let provider = value.clone().unwrap_or_default();
                let current = selected(&form.model, cx);
                let keep = if models.supports(&provider, &current) {
                    current
                } else {
                    String::new()
                };
                form.model.update(cx, |model, cx| {
                    model.set_items(model_choices(&models, &provider), window, cx);
                    model.set_selected_value(&keep, window, cx);
                });
                cx.notify();
            },
        )
        .detach();
        let agent = picker(agent_choices(catalog), &base.dev_agent, window, cx);
        let review_models = review_model_choices(catalog);
        let mut form = Self {
            script,
            fields,
            prompt,
            provider,
            model,
            agent,
            reviews: catalog
                .review_agents
                .iter()
                .map(|agent| (agent.key.clone(), agent.description.clone()))
                .collect(),
            selected: BTreeMap::new(),
            review_models,
            dry_run: base.dry_run,
            merge: base.merge,
            account: base.account.clone(),
        };
        for review in &base.review_agents {
            form.toggle_review(&review.role, review.model.as_deref(), window, cx);
        }
        form
    }

    pub fn toggle_review(
        &mut self,
        role: &str,
        model: Option<&str>,
        window: &mut Window,
        cx: &mut Context<RalphView>,
    ) {
        if self.selected.remove(role).is_none() {
            let picker = picker(self.review_models.clone(), model.unwrap_or(""), window, cx);
            self.selected.insert(role.to_owned(), picker);
        }
    }

    pub fn value(&self, name: &str, cx: &gpui_kit::App) -> String {
        self.fields[name].read(cx).value().to_string()
    }

    /// The launch this form describes, or the first input that cannot be read.
    pub fn launch(&self, cx: &gpui_kit::App) -> Result<Launch, String> {
        let iterations = if self.script.uses_iterations() {
            whole(
                &self.value("iterations", cx),
                1..=MAX_ITERATIONS,
                &format!("Iterations must be a whole number from 1 through {MAX_ITERATIONS}."),
            )?
        } else {
            3
        };
        let cycles = if self.script.uses_cycles() {
            whole(
                &self.value("cycles", cx),
                1..=MAX_CYCLES,
                &format!("Repeat cycles must be a whole number from 1 through {MAX_CYCLES}."),
            )?
        } else {
            1
        };
        let pr_number = if self.script == Script::Pr {
            Some(whole(
                &self.value("pr", cx),
                1..=u32::MAX,
                "Enter the pull request number for Ralph PR.",
            )?)
        } else {
            None
        };
        let issues = self.script == Script::Issues;
        Ok(Launch {
            repository: repository_path(&self.value("repository", cx)),
            script: self.script.clone(),
            model: selected(&self.model, cx),
            provider: selected(&self.provider, cx),
            dev_agent: selected(&self.agent, cx),
            // ralph-issues.ps1 ignores review roles: it opens no pull request.
            review_agents: if issues {
                vec![]
            } else {
                self.reviews
                    .iter()
                    .filter_map(|(role, _)| {
                        self.selected.get(role).map(|picker| ReviewAgent {
                            role: role.clone(),
                            model: Some(selected(picker, cx)).filter(|model| !model.is_empty()),
                        })
                    })
                    .collect()
            },
            iterations,
            cycles,
            branch: self.value("branch", cx).trim().to_owned(),
            prompt: self.prompt.read(cx).value().to_string(),
            pr_number,
            labels: if issues {
                self.value("labels", cx).trim().to_owned()
            } else {
                String::new()
            },
            dry_run: issues && self.dry_run,
            merge: self.script.can_merge() && self.merge,
            account: self.account.clone(),
        })
    }
}
