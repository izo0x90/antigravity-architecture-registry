//! What runs every frame before drawing: start-up, config, events, replies, the index.

use super::*;

impl CockpitApp {
    pub fn update_lifecycle(&mut self, ctx: &egui::Context) {
        if !self.initialized {
            self.initialized = true;
            ctx.set_fonts(crate::theme::font_definitions());

            self.home.connect_ws(ctx.clone());

            let reply = self.config_reply.clone();
            self.home.ui_config(ctx.clone(), move |r| {
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(r);
                }
            });

            // Auto-load dev feedback issues
            self.feedback.load_issues(&self.home, ctx.clone());
            self.open_desk(ctx);
        }

        self.apply_user_config();
        if let Some(Ok(types)) = self.desk.types_inbox.lock().ok().and_then(|mut t| t.take()) {
            self.desk.types = Some(types);
            let types = self.desk.types.clone();
            for spec in self.desk.specs() {
                spec.types.clone_from(&types);
            }
        }
        if std::mem::take(&mut self.theme_dirty) {
            crate::theme::set(self.config.theme.clone(), ctx);
            crate::graph_style::set(self.config.graph_style.clone());
            crate::status::set(self.config.status.clone());
            crate::theme::set_look(self.config.look.clone());
            crate::keymap::set_local(self.config.keymap.local());
            self.home.set_backoff(self.config.net.reconnect_min_ms, self.config.net.reconnect_max_ms);
            let (graph_ui, code_ui) = (self.config.graph.clone(), self.config.code.clone());
            for graph in self.desk.graphs() {
                graph.ui = graph_ui.clone();
            }
            for code in self.desk.codes() {
                code.ui = code_ui.clone();
                code.show_deleted = self.config.changes.deleted_in_code;
            }
            self.desk.changes.ui = self.config.changes.clone();
        }
        self.start_fresh();
        self.use_focused();

        // Every server's events, each to its desk.
        self.take_from_home(ctx);
        if std::mem::take(&mut self.desk.net.reconnected) {
            self.desk.keeper.reconnected();
            self.desk.keeper.load(&self.desk.net, ctx);
            // Back after a drop: what changed meanwhile wasn't heard.
            self.desk.graphs().for_each(GraphState::invalidate);
            self.desk.specs().for_each(SpecState::invalidate);
            self.desk.trees().for_each(ArchitectureTreeState::invalidate);
            self.desk.review.invalidate();
            self.desk.queue.invalidate();
            self.desk.details.clear();
            self.fetch_index_status(ctx);
        }
        self.take_events(ctx);
        self.tick_away(ctx);
        self.sync_inbox();
        self.link_views(ctx);
        self.take_asks(ctx);
        self.mark_out_of_date();
        self.goto_tick(ctx);
        // The focused target's relations, so the bar and popup offer only
        // the go-tos that lead somewhere.
        if let Some(id) = self.desk.view(self.focused_kind()).target().and_then(|t| t.component().cloned()) {
            let source = self.desk.graph.nav.source.clone();
            self.desk.details.want(&id, &source, &self.desk.net, ctx);
        }

        // Notes fade.
        let now = ctx.input(|i| i.time);
        match (&self.note, self.note_since) {
            (Some(_), None) => self.note_since = Some(now),
            (Some(_), Some(since)) if now - since > NOTE_SECONDS => {
                self.note = None;
                self.note_since = None;
            }
            (Some(_), Some(since)) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(NOTE_SECONDS - (now - since) + 0.05)),
            (None, _) => self.note_since = None,
        }

        self.handle_keys(ctx);
        self.quick_look_tick(ctx);
        self.use_focused();
        self.keep_state(ctx);
        self.keep_shell(ctx);
    }

    /// What the desk shown loads when it's first shown: its saved layout
    /// (the code view opens Cargo.toml when nothing was), the index's
    /// state, the component types.
    pub(super) fn open_desk(&mut self, ctx: &egui::Context) {
        self.desk.opened = true;
        self.desk.keeper.load(&self.desk.net, ctx);
        self.fetch_index_status(ctx);
        let types = self.desk.types_inbox.clone();
        self.desk.net.arch_types(ctx.clone(), move |r| {
            if let Ok(mut t) = types.lock() {
                *t = Some(r);
            }
        });
    }

    /// The desk's events and replies since last frame (every desk's, every
    /// frame: the ones not shown take theirs as the one shown, in turn).
    pub(super) fn take_events(&mut self, ctx: &egui::Context) {
        self.desk.chat.tick(&self.desk.net, ctx);
        let events = self.desk.net.incoming.lock().map(|mut lock| lock.drain(..).collect::<Vec<_>>()).unwrap_or_default();
        for event in events {
            if let harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::Index { status } } = &event {
                self.desk.index = (**status).clone();
            }
            if let harness_contracts::ClientEvent::Architecture { event } = &event {
                match event {
                    // The agent moves one graph (the one it names, else the
                    // one used last); that's a jump: back returns from it.
                    arch_graph::ArchEvent::ViewRequested { request, .. } => {
                        self.agents_graph(request.panel);
                        let before = self.jump_of(BufferKind::Graph);
                        self.desk.graph.on_event(event);
                        if self.jump_of(BufferKind::Graph) != before {
                            self.desk.jumps.record(before);
                        }
                    }
                    _ => self.desk.graphs().for_each(|g| g.on_event(event)),
                }
                self.desk.queue.on_event(event);
                // Files changed (read, or seen changing on disk): the changes too.
                if matches!(event, arch_graph::ArchEvent::Changed { .. } | arch_graph::ArchEvent::Index { .. }) {
                    self.desk.changes.invalidate();
                }
                // Only real changes refetch: a check result (often the spec
                // view's own validation) must not trigger another round.
                if matches!(event, arch_graph::ArchEvent::Changed { .. }) {
                    self.desk.specs().for_each(SpecState::invalidate);
                    self.desk.trees().for_each(ArchitectureTreeState::invalidate);
                    self.desk.review.invalidate();
                    self.desk.details.clear();
                }
            }
            self.desk.chat.process_event(event);
        }
        for reply in std::mem::take(&mut *self.desk.index_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                Ok(status) => self.desk.index = status,
                Err(e) => self.note = Some(format!("Couldn't read the index status: {e}")),
            }
        }
    }

    pub(super) fn fetch_index_status(&self, ctx: &egui::Context) {
        let inbox = self.desk.index_inbox.clone();
        self.desk.net.arch_index_status(ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push(r);
            }
        });
    }

    /// Tells the views what's out of date (they mark it, never wait for it).
    pub(super) fn mark_out_of_date(&mut self) {
        let code_stale = self.desk.code.file_stale || self.desk.index.is_stale(&self.desk.code.current_file);
        self.desk.code.out_of_date = code_stale;
        let spec_file = self.desk.spec.detail.as_ref().and_then(|d| d.component.anchor.as_ref()).map(|a| a.file.to_string());
        self.desk.spec.out_of_date = self.desk.spec.detail.as_ref().is_some_and(|d| d.stale) || spec_file.is_some_and(|f| self.desk.index.is_stale(&f));
        if self.desk.graph.stale_files != self.desk.index.stale {
            self.desk.graph.stale_files = self.desk.index.stale.clone();
        }
    }

    /// Reads the code again, as the user asked.
    pub(super) fn run_index(&mut self, op: IndexOp, ctx: &egui::Context) {
        let notes = self.desk.index_inbox.clone();
        let failed = move |r: NetResult<serde_json::Value>| {
            if let Err(e) = r
                && let Ok(mut i) = notes.lock()
            {
                i.push(Err(e));
            }
        };
        match op {
            IndexOp::Changed => self.desk.net.arch_ingest(false, ctx.clone(), failed),
            IndexOp::Rebuild => self.desk.net.arch_ingest(true, ctx.clone(), failed),
        }
    }

    /// Merges the user's UI config over the defaults once the server answers.
    pub(super) fn apply_user_config(&mut self) {
        let reply = self.config_reply.lock().ok().and_then(|mut r| r.take());
        match reply {
            Some(Ok(Some(text))) => match UiConfig::with_overrides(&text) {
                Ok(config) => {
                    self.config = config;
                    self.theme_dirty = true;
                }
                Err(err) => self.config_error = Some(err),
            },
            Some(Err(err)) => self.config_error = Some(format!("ui config: {err}")),
            Some(Ok(None)) | None => {}
        }
    }
}
