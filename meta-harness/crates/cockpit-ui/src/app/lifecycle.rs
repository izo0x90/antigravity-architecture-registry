//! What runs every frame before drawing: start-up, config, events, replies, the index.

use super::*;

impl CockpitApp {
    pub fn update_lifecycle(&mut self, ctx: &egui::Context) {
        if !self.initialized {
            self.initialized = true;
            ctx.set_fonts(crate::theme::font_definitions());

            self.net.connect_ws(ctx.clone());

            let reply = self.config_reply.clone();
            self.net.ui_config(ctx.clone(), move |r| {
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(r);
                }
            });

            // The layout as it was saved (the code view opens Cargo.toml
            // when nothing was).
            self.keeper.load(&self.net, ctx);

            // Auto-load dev feedback issues
            self.feedback.load_issues(&self.net, ctx.clone());
            self.fetch_index_status(ctx);
            let types = self.types_inbox.clone();
            self.net.arch_types(ctx.clone(), move |r| {
                if let Ok(mut t) = types.lock() {
                    *t = Some(r);
                }
            });
        }

        self.apply_user_config();
        if let Some(Ok(types)) = self.types_inbox.lock().ok().and_then(|mut t| t.take()) {
            self.types = Some(types);
            let types = self.types.clone();
            for spec in self.specs() {
                spec.types.clone_from(&types);
            }
        }
        if std::mem::take(&mut self.theme_dirty) {
            crate::theme::set(self.config.theme.clone(), ctx);
            crate::graph_style::set(self.config.graph_style.clone());
            crate::status::set(self.config.status.clone());
            crate::theme::set_look(self.config.look.clone());
            self.net.set_backoff(self.config.net.reconnect_min_ms, self.config.net.reconnect_max_ms);
            let (graph_ui, code_ui) = (self.config.graph.clone(), self.config.code.clone());
            for graph in self.graphs() {
                graph.ui = graph_ui.clone();
            }
            for code in self.codes() {
                code.ui = code_ui.clone();
            }
        }
        self.start_fresh(ctx);
        self.use_focused();

        // Poll WebSocket messages
        self.net.poll_ws(ctx);
        self.chat.tick(&self.net, ctx);
        if std::mem::take(&mut self.net.reconnected) {
            self.keeper.reconnected();
            self.keeper.load(&self.net, ctx);
            // Back after a drop: what changed meanwhile wasn't heard.
            self.graphs().for_each(GraphState::invalidate);
            self.specs().for_each(SpecState::invalidate);
            self.trees().for_each(ArchitectureTreeState::invalidate);
            self.review.invalidate();
            self.queue.invalidate();
            self.details.clear();
            self.fetch_index_status(ctx);
        }

        let events = self.net.incoming.lock().map(|mut lock| lock.drain(..).collect::<Vec<_>>()).unwrap_or_default();
        for event in events {
            if let harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::Index { status } } = &event {
                self.index = (**status).clone();
            }
            if let harness_contracts::ClientEvent::Architecture { event } = &event {
                match event {
                    // The agent moves one graph (the one it names, else the
                    // one used last); that's a jump: back returns from it.
                    arch_graph::ArchEvent::ViewRequested { request, .. } => {
                        self.agents_graph(request.column);
                        let before = self.jump_of(BufferKind::Graph);
                        self.graph.on_event(event);
                        if self.jump_of(BufferKind::Graph) != before {
                            self.jumps.record(before);
                        }
                    }
                    _ => self.graphs().for_each(|g| g.on_event(event)),
                }
                self.queue.on_event(event);
                // Only real changes refetch: a check result (often the spec
                // view's own validation) must not trigger another round.
                if matches!(event, arch_graph::ArchEvent::Changed { .. }) {
                    self.specs().for_each(SpecState::invalidate);
                    self.trees().for_each(ArchitectureTreeState::invalidate);
                    self.review.invalidate();
                    self.details.clear();
                }
            }
            self.chat.process_event(event);
        }
        for reply in std::mem::take(&mut *self.index_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                Ok(status) => self.index = status,
                Err(e) => self.note = Some(format!("Couldn't read the index status: {e}")),
            }
        }
        self.link_views(ctx);
        self.mark_out_of_date();
        self.goto_tick(ctx);
        // The focused target's relations, so the bar and popup offer only
        // the go-tos that lead somewhere.
        if let Some(id) = self.view(self.focused_kind()).target().and_then(|t| t.component().cloned()) {
            let source = self.graph.nav.source.clone();
            self.details.want(&id, &source, &self.net, ctx);
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
    }

    pub(super) fn fetch_index_status(&self, ctx: &egui::Context) {
        let inbox = self.index_inbox.clone();
        self.net.arch_index_status(ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push(r);
            }
        });
    }

    /// Tells the views what's out of date (they mark it, never wait for it).
    pub(super) fn mark_out_of_date(&mut self) {
        let code_stale = self.code.file_stale || self.index.is_stale(&self.code.current_file);
        self.code.out_of_date = code_stale;
        let spec_file = self.spec.detail.as_ref().and_then(|d| d.component.anchor.as_ref()).map(|a| a.file.to_string());
        self.spec.out_of_date = self.spec.detail.as_ref().is_some_and(|d| d.stale) || spec_file.is_some_and(|f| self.index.is_stale(&f));
        if self.graph.stale_files != self.index.stale {
            self.graph.stale_files = self.index.stale.clone();
        }
    }

    /// Reads the code again, as the user asked.
    pub(super) fn run_index(&mut self, op: IndexOp, ctx: &egui::Context) {
        let notes = self.index_inbox.clone();
        let failed = move |r: NetResult<serde_json::Value>| {
            if let Err(e) = r
                && let Ok(mut i) = notes.lock()
            {
                i.push(Err(e));
            }
        };
        match op {
            IndexOp::Changed => self.net.arch_ingest(false, ctx.clone(), failed),
            IndexOp::Rebuild => self.net.arch_ingest(true, ctx.clone(), failed),
            IndexOp::File => {
                let file = self.code.current_file.clone();
                self.net.arch_refresh(&[file], ctx.clone(), failed);
            }
        }
    }

    pub(super) fn show_code(&mut self, file: &str, start: u32, end: u32, ctx: &egui::Context) {
        self.ensure_visible(BufferKind::Code);
        self.code.show_span(file, start as usize, end as usize, &self.net, ctx.clone());
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
