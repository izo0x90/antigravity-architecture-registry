//! Calls from outside (scripts in the page, agents): run in a frame.

use super::*;

impl CockpitApp {
    /// Runs the calls from outside that came since the last frame.
    pub(super) fn serve_calls(&mut self, ctx: &egui::Context) {
        use crate::remote::Call;
        for (call, respond) in self.desk.remote.take(ctx, &self.desk.net) {
            match call {
                Call::Read => self.desk.remote.answer_screen(respond),
                Call::Actions => respond(Ok(self.actions_listed())),
                Call::Run { id, value } => match self.run_named(&id, value, ctx) {
                    Ok(()) => self.desk.remote.answer_ran(respond),
                    Err(e) => respond(Err(e)),
                },
                Call::Click { text, x, y } => self.desk.remote.click(text, x.zip(y).map(|(x, y)| egui::pos2(x, y)), respond),
                Call::Type { text } => self.desk.remote.type_text(text, respond),
                Call::Keys { keys } => self.desk.remote.keys(&keys, respond),
            }
        }
    }

    /// Everything that can be run now, as a call from outside lists it.
    pub(super) fn actions_listed(&self) -> serde_json::Value {
        let situation = self.situation();
        let listed: Vec<serde_json::Value> = self
            .palette_actions()
            .iter()
            .map(|b| {
                let (needs, choices) = match &b.action.param {
                    Param::None => ("nothing", Vec::new()),
                    Param::Choice(_) => ("a choice", b.action.param.choices()),
                    Param::Toggle { .. } => ("on or off", b.action.param.choices()),
                    Param::Text { .. } => ("text", Vec::new()),
                    Param::Search { .. } => ("a search", Vec::new()),
                };
                serde_json::json!({
                    "id": b.action.id,
                    "label": b.action.label,
                    "detail": b.action.detail,
                    "view": match b.origin { Origin::Shell => "shell", Origin::View(kind) => kind.name() },
                    "keys": b.key_hint(),
                    "needs": needs,
                    "choices": choices.iter().map(|c| serde_json::json!({ "value": c.value, "label": c.label, "current": c.current })).collect::<Vec<_>>(),
                    "available": b.action.when.holds(&situation),
                })
            })
            .collect();
        serde_json::Value::Array(listed)
    }

    /// Runs an action by id as its key would, with the value given (a
    /// choice by value or label); says why not if it can't.
    pub(super) fn run_named(&mut self, id: &str, value: Option<String>, ctx: &egui::Context) -> Result<(), String> {
        let bound = self.palette_actions().into_iter().find(|b| b.action.id == id).ok_or_else(|| format!("no action {id:?} now (actions lists them)"))?;
        if !bound.action.when.holds(&self.situation()) {
            return Err(format!("{id:?} doesn't apply here now"));
        }
        let value = match (&bound.action.param, value) {
            (Param::None, _) => None,
            (Param::Text { .. } | Param::Search { .. }, Some(v)) => Some(v),
            (param @ (Param::Choice(_) | Param::Toggle { .. }), Some(v)) => {
                let choices = param.choices();
                let wanted = match v.as_str() {
                    "on" => "true",
                    "off" => "false",
                    other => other,
                };
                let found = choices.iter().find(|c| c.value == wanted).or_else(|| choices.iter().find(|c| c.label.eq_ignore_ascii_case(wanted)));
                Some(found.map(|c| c.value.clone()).ok_or_else(|| format!("{id:?}: {v:?} isn't one of {:?}", choices.iter().map(|c| &c.value).collect::<Vec<_>>()))?)
            }
            (_, None) => return Err(format!("{id:?} needs a value (actions shows what)")),
        };
        self.note = None;
        self.invoke(Invocation { action: bound.action, origin: bound.origin, value }, ctx);
        Ok(())
    }

    /// The app's state as a read from outside reports it.
    pub(super) fn remote_status(&self) -> crate::remote::Status {
        let focused = self.desk.columns.focused();
        crate::remote::Status {
            mode: format!("{:?}", self.input_mode),
            columns: self.desk.columns.panes().iter().enumerate().map(|(i, p)| serde_json::json!({ "column": i + 1, "view": self.describe(*p), "focused": i == focused })).collect(),
            note: self.note.clone(),
            leader: (self.input_mode == InputMode::Leader).then(|| self.leader_path.iter().collect()),
            palette: self.palette.is_open(),
        }
    }
}
