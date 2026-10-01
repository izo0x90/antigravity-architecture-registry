// Antigravity Meta-Harness Cockpit Frontend Controller
(() => {
  'use strict';

  // --- Official Antigravity ACP Models & Defaults ---
  const DEFAULT_MODELS = [
    { slug: 'gemini-3.7-flash-high', name: 'Gemini 3.7 Flash (High Effort)', family: 'gemini-3.7-flash', effort: 'high' },
    { slug: 'gemini-3.7-flash-medium', name: 'Gemini 3.7 Flash (Med Effort)', family: 'gemini-3.7-flash', effort: 'medium' },
    { slug: 'gemini-3.7-flash-low', name: 'Gemini 3.7 Flash (Low Effort)', family: 'gemini-3.7-flash', effort: 'low' },
    { slug: 'gemini-3.8-flash-high', name: 'Gemini 3.8 Flash (High Effort)', family: 'gemini-3.8-flash', effort: 'high' },
    { slug: 'gemini-3.8-flash-medium', name: 'Gemini 3.8 Flash (Med Effort)', family: 'gemini-3.8-flash', effort: 'medium' },
    { slug: 'gemini-3.8-flash-low', name: 'Gemini 3.8 Flash (Low Effort)', family: 'gemini-3.8-flash', effort: 'low' },
    { slug: 'gemini-pro-agent', name: 'Gemini 3.1 Pro (High Effort)', family: 'gemini-pro', effort: 'high' },
    { slug: 'gemini-3.1-pro-low', name: 'Gemini 3.1 Pro (Low Effort)', family: 'gemini-pro', effort: 'low' },
    { slug: 'gemini-3.6-flash-high', name: 'Gemini 3.6 Flash (High Effort)', family: 'gemini-3.6-flash', effort: 'high' },
    { slug: 'gemini-3.6-flash-medium', name: 'Gemini 3.6 Flash (Med Effort)', family: 'gemini-3.6-flash', effort: 'medium' },
    { slug: 'gemini-3.6-flash-low', name: 'Gemini 3.6 Flash (Low Effort)', family: 'gemini-3.6-flash', effort: 'low' }
  ];

  // --- Central Application State ---
  const state = {
    connected: false,
    ws: null,
    activeLayout: 3, // 1, 2, or 3 columns
    activeSlots: ['graph', 'chat', 'code'],
    focusedSlot: 0,
    inspectMode: false,
    inspectTarget: null,
    availableModels: DEFAULT_MODELS,

    // Buffers Data Store (Persistent across view swaps)
    buffers: {
      chat: {
        threadId: null,
        messages: [],
        streamingMessageId: null,
        selectedModel: 'gemini-3.7-flash-high',
        selectedEffort: 'high',
        runtimeMode: 'auto_edit',
        planMode: false,
        isStreaming: false
      },
      code: {
        path: 'Cargo.toml',
        content: '',
        lines: 0,
        highlightLine: null,
        comments: {}
      },
      graph: {
        scale: 1,
        panX: 0,
        panY: 0,
        isPanning: false,
        lastMouseX: 0,
        lastMouseY: 0,
        selectedNodeId: null,
        nodes: [
          { id: 'driver', label: 'AntigravityDriver', type: 'Core Engine', x: 50, y: 80, w: 220, h: 100, inputs: ['credentials', 'cli_path'], outputs: ['catalog', 'session_lease'] },
          { id: 'adapter', label: 'AntigravityAdapter', type: 'Session Orchestrator', x: 340, y: 80, w: 230, h: 110, inputs: ['turn_prompt', 'steer_cmd'], outputs: ['runtime_events', 'tool_approvals'] },
          { id: 'transport', label: 'JsonRpcTransport', type: 'Stdio Pipe Engine', x: 640, y: 80, w: 220, h: 100, inputs: ['stdio_in'], outputs: ['rpc_frames', 'reverse_rpc'] },
          { id: 'fsproxy', label: 'FsProxy Sandbox', type: 'Security Barrier', x: 640, y: 260, w: 220, h: 90, inputs: ['raw_path'], outputs: ['canonical_safe_path'] },
          { id: 'server', label: 'Axum Control Plane', type: 'HTTP / WS Daemon', x: 340, y: 260, w: 230, h: 90, inputs: ['client_req', 'ws_stream'], outputs: ['broadcast_stream'] }
        ],
        edges: [
          { from: 'driver', to: 'adapter', label: 'driver_lease' },
          { from: 'adapter', to: 'transport', label: 'stdio_rpc' },
          { from: 'transport', to: 'fsproxy', label: 'fs_check' },
          { from: 'server', to: 'adapter', label: 'session_ctrl' }
        ]
      },
      tree: {
        root: {
          id: 'root',
          label: 'Meta-Harness Control System Plan',
          type: 'ROOT',
          status: 'ACTIVE',
          expanded: true,
          children: [
            {
              id: 'plan_process',
              label: 'Process Execution & Stdio Pipes',
              type: 'REQUIREMENT',
              status: 'PASS',
              detail: 'TokioProcessSpawner running real agy binary over async stdio',
              expanded: false
            },
            {
              id: 'plan_transport',
              label: 'Transport & Protocol Engine',
              type: 'REQUIREMENT',
              status: 'PASS',
              detail: 'Non-blocking line-delimited JSON-RPC with reverse-RPC support',
              expanded: false
            },
            {
              id: 'plan_sandboxing',
              label: 'Filesystem Sandboxing & Traversal Guard',
              type: 'INVARIANT',
              status: 'PASS',
              detail: 'FsProxy canonicalizes paths and rejects symlink/directory escapes',
              expanded: false
            },
            {
              id: 'plan_cockpit',
              label: 'Web Cockpit & Navigation Ergonomics',
              type: 'INVARIANT',
              status: 'ACTIVE',
              detail: 'Multi-column adaptive tiling, instant hotkey buffer swap, element inspector',
              expanded: true,
              children: [
                { id: 'cockpit_chat', label: 'Real-time WS Chat & Steering', type: 'LEAF', status: 'PASS' },
                { id: 'cockpit_code', label: 'Code Viewer with Auto-Open Links', type: 'LEAF', status: 'PASS' },
                { id: 'cockpit_graph', label: 'Deterministic DAG Architecture View', type: 'LEAF', status: 'PASS' },
                { id: 'cockpit_feedback', label: 'In-App Feedback JSON Collector', type: 'LEAF', status: 'PASS' }
              ]
            }
          ]
        }
      }
    }
  };

  // --- WebSocket Connection & Event Streaming ---
  function initWebSocket() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/ws/events`;
    const pill = document.getElementById('connection-pill');
    const label = document.getElementById('connection-label');

    try {
      const ws = new WebSocket(wsUrl);

      ws.onopen = () => {
        state.connected = true;
        state.ws = ws;
        pill.className = 'status-pill status-connected';
        label.textContent = 'ONLINE';
        showToast('Connected to Meta-Harness Daemon');
        ensureSessionStarted();
      };

      ws.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data);
          handleRuntimeEvent(payload);
        } catch (e) {
          console.error('Failed to parse WS event:', e);
        }
      };

      ws.onclose = () => {
        state.connected = false;
        pill.className = 'status-pill status-disconnected';
        label.textContent = 'OFFLINE';
        setTimeout(initWebSocket, 2500);
      };

      ws.onerror = () => {
        pill.className = 'status-pill status-disconnected';
        label.textContent = 'ERROR';
      };
    } catch (e) {
      console.warn('WS Init failed:', e);
      setTimeout(initWebSocket, 3000);
    }
  }

  // Auto-start ACP session if not present
  async function ensureSessionStarted() {
    if (state.buffers.chat.threadId) return;
    try {
      const threadId = 'session-' + Date.now();
      const res = await fetch('/api/sessions/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          thread_id: threadId,
          cwd: '.',
          runtime_mode: state.buffers.chat.runtimeMode,
          model: state.buffers.chat.selectedModel
        })
      });
      if (res.ok) {
        state.buffers.chat.threadId = threadId;
        const sdata = await res.json().catch(() => null);
        if (sdata && sdata.model) {
          state.buffers.chat.selectedModel = sdata.model;
        }
        console.log('Session started:', threadId);
        addChatMessage('assistant', 'Meta-Harness Control Daemon initialized. Ready to orchestrate sessions.');
      } else {
        const errText = await res.text();
        console.error('Session start failed:', errText);
        addChatMessage('assistant', `⚠️ Failed to start ACP session: ${errText}`);
      }
    } catch (e) {
      console.error('Failed to start session:', e);
      addChatMessage('assistant', `⚠️ Connection error starting session: ${e.message}`);
    }
  }

  // --- Streaming Event Dispatcher ---
  function handleRuntimeEvent(event) {
    const chat = state.buffers.chat;
    const type = event.type || '';

    if (type === 'content.delta') {
      const payload = event.payload || {};
      const isReasoning = payload.stream_kind === 'reasoning_text';
      const delta = payload.delta || '';
      appendAssistantDelta(delta, isReasoning);
      if (!isReasoning) {
        detectAndAutoOpenFile(delta);
      }
    } else if (type === 'task.started' || type === 'task.progress' || type === 'task.updated' || type === 'task.completed') {
      const p = event.payload || {};
      renderToolCall({
        id: p.task_id || event.turn_id,
        title: p.title || p.task_type || 'Task',
        status: p.status || type.split('.')[1]
      });
    } else if (type === 'request.opened') {
      const p = event.payload || {};
      renderApprovalRequest(event.request_id, p.tool_call || {}, p.options || []);
    } else if (type === 'turn.started') {
      chat.isStreaming = true;
    } else if (type === 'turn.completed') {
      chat.isStreaming = false;
      chat.streamingMessageId = null;
      renderChatPane();
    }
  }

  function appendAssistantDelta(delta, isThought) {
    const chat = state.buffers.chat;
    if (!chat.streamingMessageId) {
      const msgId = 'msg-' + Date.now();
      chat.streamingMessageId = msgId;
      chat.messages.push({
        id: msgId,
        role: 'assistant',
        content: isThought ? '' : delta,
        thought: isThought ? delta : '',
        timestamp: new Date().toLocaleTimeString()
      });
    } else {
      const msg = chat.messages.find(m => m.id === chat.streamingMessageId);
      if (msg) {
        if (isThought) {
          msg.thought = (msg.thought || '') + delta;
        } else {
          msg.content = (msg.content || '') + delta;
        }
      }
    }
    renderChatPane();
  }

  function renderToolCall(tool) {
    const chat = state.buffers.chat;
    const existing = chat.messages.find(m => m.toolId === tool.id);
    if (existing) {
      existing.status = tool.status;
      existing.toolName = tool.title || existing.toolName;
    } else {
      chat.messages.push({
        id: 'tool-' + Date.now(),
        toolId: tool.id,
        role: 'tool',
        toolName: tool.title || 'Tool Call',
        status: tool.status || 'running',
        timestamp: new Date().toLocaleTimeString()
      });
    }
    renderChatPane();
  }

  function renderApprovalRequest(requestId, toolCall, options) {
    const chat = state.buffers.chat;
    chat.messages.push({
      id: 'req-' + requestId,
      role: 'approval',
      requestId,
      title: toolCall.title || toolCall.kind || 'Permission Required',
      details: toolCall.data ? JSON.stringify(toolCall.data) : (toolCall.status || 'Action requires confirmation'),
      options: options.length ? options : [{ decision: 'accept', label: 'Approve' }, { decision: 'decline', label: 'Decline' }],
      timestamp: new Date().toLocaleTimeString()
    });
    renderChatPane();
  }

  async function resolveApproval(requestId, decision) {
    const chat = state.buffers.chat;
    if (!chat.threadId) return;
    try {
      const res = await fetch(`/api/sessions/${chat.threadId}/approvals/${requestId}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ decision })
      });
      if (res.ok) {
        showToast(`Permission: ${decision}`);
        chat.messages = chat.messages.filter(m => m.requestId !== requestId);
        renderChatPane();
      }
    } catch (e) {
      showToast('Approval resolution failed: ' + e.message);
    }
  }

  async function loadModelsFromDriver() {
    try {
      const res = await fetch('/api/driver/snapshot');
      if (res.ok) {
        const snap = await res.json();
        if (snap.models && snap.models.length > 0) {
          state.availableModels = snap.models.map(m => {
            const slug = m.slug;
            let effort = 'high';
            if (slug.endsWith('-low')) effort = 'low';
            else if (slug.endsWith('-medium') || slug.endsWith('-med')) effort = 'medium';

            let family = 'gemini-3.7-flash';
            if (slug.startsWith('gemini-3.8-flash')) family = 'gemini-3.8-flash';
            else if (slug.startsWith('gemini-3.6-flash')) family = 'gemini-3.6-flash';
            else if (slug.startsWith('gemini-pro') || slug.startsWith('gemini-3.1-pro')) family = 'gemini-pro';

            return {
              slug,
              name: m.name || slug,
              family,
              effort
            };
          });
          if (snap.default_model) {
            state.buffers.chat.selectedModel = snap.default_model;
          }
          renderChatPane();
        }
      }
    } catch (e) {
      console.warn('Driver snapshot unavailable for model discovery:', e);
    }
  }

  function addChatMessage(role, content) {
    const chat = state.buffers.chat;
    chat.messages.push({
      id: 'msg-' + Date.now(),
      role,
      content,
      timestamp: new Date().toLocaleTimeString()
    });
    renderChatPane();
  }


  // --- Auto-Open File Link Detection ---
  function detectAndAutoOpenFile(text) {
    if (!text) return;
    // Regex matching local file paths or markdown file links
    const fileMatch = text.match(/(?:file:\/\/)?([a-zA-Z0-9_\-./]+\.(?:rs|toml|json|ts|js|md|html|css))(?::(\d+)|#L(\d+))?/);
    if (fileMatch) {
      const filePath = fileMatch[1];
      const line = fileMatch[2] || fileMatch[3] ? parseInt(fileMatch[2] || fileMatch[3], 10) : null;
      console.log('Auto-detected file reference:', filePath, 'Line:', line);
      loadCodeFile(filePath, line);
    }
  }

  // --- REST Actions ---
  async function sendTurnPrompt(prompt) {
    if (!prompt.trim()) return;
    const chat = state.buffers.chat;
    await ensureSessionStarted();
    if (!chat.threadId) {
      addChatMessage('assistant', '⚠️ Cannot send message: active ACP session could not be established.');
      return;
    }

    addChatMessage('user', prompt);
    chat.isStreaming = true;

    try {
      const res = await fetch(`/api/sessions/${chat.threadId}/turn`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ prompt })
      });
      if (!res.ok) {
        const errText = await res.text();
        addChatMessage('assistant', `⚠️ Turn error: ${errText || res.statusText}`);
      }
    } catch (e) {
      addChatMessage('assistant', `⚠️ Failed to send prompt: ${e.message}`);
    }
  }

  async function cancelActiveTurn() {
    const chat = state.buffers.chat;
    if (!chat.threadId) return;
    try {
      await fetch(`/api/sessions/${chat.threadId}/cancel`, { method: 'POST' });
      showToast('In-flight turn cancelled');
    } catch (e) {
      showToast('Failed to cancel turn: ' + e.message);
    }
  }

  async function steerSession(model, runtimeMode) {
    const chat = state.buffers.chat;
    if (!chat.threadId) return;
    try {
      const res = await fetch(`/api/sessions/${chat.threadId}/steer`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ model, runtime_mode: runtimeMode })
      });
      if (res.ok) {
        showToast(`Model: ${model} | Mode: ${runtimeMode}`);
      } else {
        const err = await res.text();
        showToast('Steering failed: ' + err);
      }
    } catch (e) {
      showToast('Steering failed: ' + e.message);
    }
  }

  // --- Code Viewer Loader ---
  async function loadCodeFile(filePath, highlightLine = null) {
    try {
      const res = await fetch(`/api/fs/read?path=${encodeURIComponent(filePath)}`);
      if (!res.ok) {
        showToast(`Could not open ${filePath}: ${res.statusText}`);
        return;
      }
      const data = await res.json();
      state.buffers.code.path = data.path;
      state.buffers.code.content = data.content;
      state.buffers.code.lines = data.lines;
      state.buffers.code.highlightLine = highlightLine;

      // If Code buffer isn't currently visible on desktop or mobile, make it active
      if (!state.activeSlots.includes('code')) {
        state.activeSlots[2] = 'code';
      }

      renderSlots();
      showToast(`Loaded ${filePath} (${data.lines} lines)`);

      // Scroll to line
      if (highlightLine) {
        setTimeout(() => {
          const el = document.getElementById(`line-${highlightLine}`);
          if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }, 100);
      }
    } catch (e) {
      showToast(`Error reading file: ${e.message}`);
    }
  }

  // --- Pane Renderers ---

  function renderChatPane() {
    const chat = state.buffers.chat;
    const container = document.getElementById('chat-pane-container');
    if (!container) return;

    const messagesHtml = chat.messages.map(m => {
      if (m.role === 'approval') {
        const btns = (m.options || []).map(opt =>
          `<button class="btn-approve-action" data-req-id="${m.requestId}" data-dec="${opt.decision}">${escapeHtml(opt.label)}</button>`
        ).join(' ');
        return `
          <div class="tool-card approval-card" data-inspect-id="approval_${m.requestId}" style="border-left: 3px solid #f59e0b; background: rgba(245, 158, 11, 0.08);">
            <div class="tool-header">
              <span>🛡️ ${escapeHtml(m.title)}</span>
              <span class="tool-status" style="color: #f59e0b;">ACTION REQUIRED</span>
            </div>
            <div style="font-size: 11px; margin: 6px 0; color: var(--text-secondary); word-break: break-all;">${escapeHtml(m.details)}</div>
            <div style="display:flex; gap:6px; margin-top:8px;">${btns}</div>
          </div>
        `;
      }

      if (m.role === 'tool') {
        return `
          <div class="tool-card" data-inspect-id="tool_card_${m.id}">
            <div class="tool-header">
              <span>🔧 ${m.toolName}</span>
              <span class="tool-status">${m.status}</span>
            </div>
            <div class="tool-time">${m.timestamp}</div>
          </div>
        `;
      }

      const isUser = m.role === 'user';
      let thoughtHtml = '';
      if (m.thought) {
        thoughtHtml = `
          <details class="thought-box" open data-inspect-id="thought_box_${m.id}">
            <summary>Thinking (${m.thought.length} chars)</summary>
            <div class="thought-content">${escapeHtml(m.thought)}</div>
          </details>
        `;
      }

      return `
        <div class="message-bubble ${isUser ? 'message-user' : 'message-assistant'}" data-inspect-id="message_${m.id}">
          ${thoughtHtml}
          <div class="message-text">${escapeHtml(m.content || '')}</div>
        </div>
      `;
    }).join('');

    const modelOptions = (state.availableModels && state.availableModels.length > 0)
      ? state.availableModels.map(m =>
          `<option value="${escapeHtml(m.slug)}" ${chat.selectedModel === m.slug ? 'selected' : ''}>${escapeHtml(m.name || m.slug)}</option>`
        ).join('')
      : DEFAULT_MODELS.map(m =>
          `<option value="${escapeHtml(m.slug)}" ${chat.selectedModel === m.slug ? 'selected' : ''}>${escapeHtml(m.name)}</option>`
        ).join('');

    const effortValue = chat.selectedEffort || (chat.selectedModel.endsWith('-low') ? 'low' : chat.selectedModel.endsWith('-medium') ? 'medium' : 'high');

    container.innerHTML = `
      <div class="chat-pane" data-inspect-id="chat_pane">
        <div class="chat-controls-bar">
          <div style="display:flex; gap:6px; align-items:center; flex-wrap:wrap;">
            <select id="chat-model-select" class="chat-select" title="Select Antigravity Model" data-inspect-id="chat_model_select">
              ${modelOptions}
            </select>
            <select id="chat-effort-select" class="chat-select" title="Reasoning Effort Mode" data-inspect-id="chat_effort_select">
              <option value="high" ${effortValue === 'high' ? 'selected' : ''}>🧠 High Effort</option>
              <option value="medium" ${effortValue === 'medium' ? 'selected' : ''}>🧠 Med Effort</option>
              <option value="low" ${effortValue === 'low' ? 'selected' : ''}>🧠 Low Effort</option>
            </select>
            <select id="chat-mode-select" class="chat-select" title="Tool Permission Mode" data-inspect-id="chat_mode_select">
              <option value="auto_edit" ${chat.runtimeMode === 'auto_edit' || chat.runtimeMode === 'auto-accept-edits' ? 'selected' : ''}>⚡ Auto-Accept Edits</option>
              <option value="default" ${chat.runtimeMode === 'default' || chat.runtimeMode === 'ask-on-destructive' ? 'selected' : ''}>🛡️ Ask On Tools</option>
              <option value="yolo" ${chat.runtimeMode === 'yolo' || chat.runtimeMode === 'full-access' ? 'selected' : ''}>🔥 YOLO (Full Access)</option>
            </select>
            <button type="button" id="btn-plan-toggle" class="btn-plan-mode ${chat.planMode ? 'active' : ''}" title="Toggle Plan Mode (auto-generates implementation plan before modifying files)" data-inspect-id="btn_plan_toggle">
              📋 ${chat.planMode ? 'Plan Active' : 'Plan Mode'}
            </button>
          </div>
          <button id="btn-cancel-turn" class="btn-cancel-turn" title="Stop Active Turn" data-inspect-id="btn_cancel_turn">Stop</button>
        </div>

        <div class="chat-messages" id="chat-messages-list">
          ${messagesHtml}
        </div>

        <div class="chat-command-chips">
          <span class="chip-label">Quick:</span>
          <span class="chip-cmd ${chat.planMode ? 'active' : ''}" id="chip-cmd-plan" title="Click to toggle Plan Mode">/plan</span>
          <span class="chip-cmd" id="chip-cmd-logout" title="Click to sign out of Google account">/logout</span>
        </div>

        <form id="chat-input-form" class="chat-input-bar" data-inspect-id="chat_input_bar">
          <input type="text" id="chat-prompt-input" class="chat-input" placeholder="${chat.planMode ? '📋 Plan Mode active: Describe feature to generate implementation plan...' : 'Enter prompt or instruction...'}" autocomplete="off" required>
          <button type="submit" class="btn-send" data-inspect-id="btn_chat_send">Send</button>
        </form>
      </div>
    `;

    // Auto-scroll messages to bottom
    const listEl = document.getElementById('chat-messages-list');
    if (listEl) listEl.scrollTop = listEl.scrollHeight;

    // Attach approval buttons handlers
    container.querySelectorAll('.btn-approve-action').forEach(btn => {
      btn.onclick = () => {
        const reqId = btn.getAttribute('data-req-id');
        const dec = btn.getAttribute('data-dec');
        resolveApproval(reqId, dec);
      };
    });

    // Attach event listeners
    const form = document.getElementById('chat-input-form');
    if (form) {
      form.onsubmit = (e) => {
        e.preventDefault();
        const input = document.getElementById('chat-prompt-input');
        if (input && input.value) {
          let val = input.value.trim();
          input.value = '';
          if (chat.planMode && !val.startsWith('/plan')) {
            val = `/plan ${val}`;
          }
          sendTurnPrompt(val);
        }
      };
    }

    const cancelBtn = document.getElementById('btn-cancel-turn');
    if (cancelBtn) cancelBtn.onclick = cancelActiveTurn;

    const modelSelect = document.getElementById('chat-model-select');
    const effortSelect = document.getElementById('chat-effort-select');

    if (modelSelect) {
      modelSelect.onchange = (e) => {
        chat.selectedModel = e.target.value;
        if (chat.selectedModel.endsWith('-low')) chat.selectedEffort = 'low';
        else if (chat.selectedModel.endsWith('-medium') || chat.selectedModel.endsWith('-med')) chat.selectedEffort = 'medium';
        else chat.selectedEffort = 'high';

        if (effortSelect) effortSelect.value = chat.selectedEffort;
        steerSession(chat.selectedModel, chat.runtimeMode);
      };
    }

    if (effortSelect) {
      effortSelect.onchange = (e) => {
        const effort = e.target.value;
        chat.selectedEffort = effort;

        let currentFamily = 'gemini-3.7-flash';
        if (chat.selectedModel.startsWith('gemini-3.8-flash')) currentFamily = 'gemini-3.8-flash';
        else if (chat.selectedModel.startsWith('gemini-3.6-flash')) currentFamily = 'gemini-3.6-flash';
        else if (chat.selectedModel.startsWith('gemini-pro') || chat.selectedModel.startsWith('gemini-3.1-pro')) currentFamily = 'gemini-pro';

        let newModel = null;
        if (currentFamily === 'gemini-pro') {
          newModel = effort === 'low' ? 'gemini-3.1-pro-low' : 'gemini-pro-agent';
        } else {
          newModel = `${currentFamily}-${effort}`;
        }

        chat.selectedModel = newModel;
        if (modelSelect) modelSelect.value = chat.selectedModel;
        steerSession(chat.selectedModel, chat.runtimeMode);
      };
    }

    const modeSelect = document.getElementById('chat-mode-select');
    if (modeSelect) {
      modeSelect.onchange = (e) => {
        chat.runtimeMode = e.target.value;
        steerSession(chat.selectedModel, chat.runtimeMode);
      };
    }

    const planBtn = document.getElementById('btn-plan-toggle');
    if (planBtn) {
      planBtn.onclick = () => {
        chat.planMode = !chat.planMode;
        planBtn.classList.toggle('active', chat.planMode);
        planBtn.innerHTML = `📋 ${chat.planMode ? 'Plan Active' : 'Plan Mode'}`;
        const input = document.getElementById('chat-prompt-input');
        if (input) {
          input.placeholder = chat.planMode
            ? '📋 Plan Mode active: Describe feature to generate implementation plan...'
            : 'Enter prompt or instruction...';
        }
        const chipPlan = document.getElementById('chip-cmd-plan');
        if (chipPlan) chipPlan.classList.toggle('active', chat.planMode);
        showToast(chat.planMode ? 'Plan Mode active: Prompts will trigger /plan' : 'Plan Mode disabled');
      };
    }

    const chipPlan = document.getElementById('chip-cmd-plan');
    if (chipPlan) {
      chipPlan.onclick = () => {
        if (planBtn) planBtn.click();
      };
    }

    const chipLogout = document.getElementById('chip-cmd-logout');
    if (chipLogout) {
      chipLogout.onclick = () => {
        if (confirm('Disconnect active session credentials (/logout)?')) {
          sendTurnPrompt('/logout');
        }
      };
    }
  }


  function renderCodePane() {
    const code = state.buffers.code;
    const container = document.getElementById('code-pane-container');
    if (!container) return;

    const lines = (code.content || '// No file loaded').split('\n');
    let gutterHtml = '';
    let bodyHtml = '';

    lines.forEach((line, idx) => {
      const lineNum = idx + 1;
      const isHigh = lineNum === code.highlightLine;
      const comment = code.comments[lineNum];
      
      gutterHtml += `
        <div class="gutter-line" data-line="${lineNum}">
          <span class="comment-tag" title="Add comment">💬</span>
          <span>${lineNum}</span>
        </div>
      `;

      let commentHtml = '';
      if (comment) {
        commentHtml = `<div class="code-comment-box">💬 <strong>Review Note:</strong> ${escapeHtml(comment)}</div>`;
      }

      bodyHtml += `
        <div class="code-line ${isHigh ? 'line-highlight' : ''}" id="line-${lineNum}" data-inspect-id="code_line_${lineNum}">
          ${escapeHtml(line)}
          ${commentHtml}
        </div>
      `;
    });

    container.innerHTML = `
      <div class="code-pane" data-inspect-id="code_pane">
        <div class="code-header">
          <div class="code-path-badge">
            <span>📄</span>
            <span>${escapeHtml(code.path)}</span>
          </div>
          <div class="code-actions">
            <button class="btn-preset" id="btn-reload-code" title="Reload File">Reload</button>
          </div>
        </div>
        <div class="code-viewport">
          <div class="code-gutter">${gutterHtml}</div>
          <div class="code-body">${bodyHtml}</div>
        </div>
      </div>
    `;

    const reloadBtn = document.getElementById('btn-reload-code');
    if (reloadBtn) {
      reloadBtn.onclick = () => loadCodeFile(code.path, code.highlightLine);
    }
  }

  function renderGraphPane() {
    const g = state.buffers.graph;
    const container = document.getElementById('graph-pane-container');
    if (!container) return;

    // Build SVG Elements
    const nodesHtml = g.nodes.map(n => {
      const isSelected = n.id === g.selectedNodeId;
      return `
        <g class="graph-node" data-node-id="${n.id}" data-inspect-id="graph_node_${n.id}" transform="translate(${n.x}, ${n.y})">
          <rect class="node-box" width="${n.w}" height="${n.h}" rx="8" ${isSelected ? 'style="stroke:var(--accent-cyan); stroke-width:2.5px;"' : ''}></rect>
          <rect class="node-header" width="${n.w}" height="28" rx="8"></rect>
          <text class="node-title" x="12" y="19">${n.label}</text>
          <text class="node-badge" x="12" y="44">[${n.type}]</text>
          
          <!-- Ports -->
          ${n.inputs.map((inp, idx) => `
            <circle class="node-port" cx="0" cy="${60 + idx * 16}" r="4"></circle>
            <text x="8" y="${64 + idx * 16}" font-size="9" fill="#94a3b8">${inp}</text>
          `).join('')}

          ${n.outputs.map((out, idx) => `
            <circle class="node-port" cx="${n.w}" cy="${60 + idx * 16}" r="4"></circle>
            <text x="${n.w - 8}" y="${64 + idx * 16}" text-anchor="end" font-size="9" fill="#38bdf8">${out}</text>
          `).join('')}
        </g>
      `;
    }).join('');

    const edgesHtml = g.edges.map(e => {
      const fromNode = g.nodes.find(n => n.id === e.from);
      const toNode = g.nodes.find(n => n.id === e.to);
      if (!fromNode || !toNode) return '';

      const x1 = fromNode.x + fromNode.w;
      const y1 = fromNode.y + fromNode.h / 2;
      const x2 = toNode.x;
      const y2 = toNode.y + toNode.h / 2;
      const dx = (x2 - x1) / 2;
      const pathD = `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;

      return `
        <g data-inspect-id="graph_edge_${e.from}_${e.to}">
          <path d="${pathD}" class="graph-edge active"></path>
          <circle cx="${(x1+x2)/2}" cy="${(y1+y2)/2}" r="3" fill="#38bdf8"></circle>
        </g>
      `;
    }).join('');

    container.innerHTML = `
      <div class="graph-pane" data-inspect-id="graph_pane">
        <div class="graph-canvas-container" id="graph-viewport">
          <svg class="graph-svg" id="graph-svg-root" viewBox="0 0 950 420">
            <defs>
              <pattern id="grid" width="20" height="20" patternUnits="userSpaceOnUse">
                <circle cx="2" cy="2" r="1" fill="#1e293b"></circle>
              </pattern>
            </defs>
            <rect width="100%" height="100%" fill="url(#grid)"></rect>
            ${edgesHtml}
            ${nodesHtml}
          </svg>
        </div>

        <div class="graph-controls">
          <button class="btn-graph-ctrl" id="btn-graph-zoom-in" title="Zoom In">+</button>
          <button class="btn-graph-ctrl" id="btn-graph-zoom-out" title="Zoom Out">-</button>
          <button class="btn-graph-ctrl" id="btn-graph-reset" title="Reset View">⟲</button>
        </div>
      </div>
    `;

    // Node click to inspect
    container.querySelectorAll('.graph-node').forEach(nodeEl => {
      nodeEl.onclick = () => {
        const id = nodeEl.getAttribute('data-node-id');
        g.selectedNodeId = id;
        renderGraphPane();
        showToast(`Selected Node: ${id}`);
      };
    });
  }

  function renderTreePane() {
    const t = state.buffers.tree;
    const container = document.getElementById('tree-pane-container');
    if (!container) return;

    function renderNode(node, depth = 0) {
      const hasChildren = node.children && node.children.length > 0;
      const isPass = node.status === 'PASS';
      const badgeClass = isPass ? 'badge-pass' : 'badge-pending';
      
      let html = `
        <div class="tree-row" style="padding-left: ${depth * 18 + 8}px;" data-node-id="${node.id}" data-inspect-id="tree_node_${node.id}">
          <span class="tree-toggle">${hasChildren ? (node.expanded ? '▼' : '▶') : '•'}</span>
          <span class="tree-icon">${node.type === 'ROOT' ? '⚡' : node.type === 'INVARIANT' ? '🛡️' : '📦'}</span>
          <span class="tree-label">${escapeHtml(node.label)}</span>
          <span class="tree-badge ${badgeClass}">${node.status}</span>
        </div>
      `;

      if (hasChildren && node.expanded) {
        html += node.children.map(child => renderNode(child, depth + 1)).join('');
      }
      return html;
    }

    container.innerHTML = `
      <div class="tree-pane" data-inspect-id="tree_pane">
        <div class="slot-header">
          <span class="slot-title">🌲 STRUCTURED PLAN & INVARIANTS</span>
        </div>
        <div class="tree-viewport">
          ${renderNode(t.root)}
        </div>
      </div>
    `;

    // Toggle child nodes
    container.querySelectorAll('.tree-row').forEach(row => {
      row.onclick = () => {
        const nodeId = row.getAttribute('data-node-id');
        toggleTreeNode(t.root, nodeId);
        renderTreePane();
      };
    });
  }

  function toggleTreeNode(parent, id) {
    if (parent.id === id) {
      parent.expanded = !parent.expanded;
      return true;
    }
    if (parent.children) {
      for (const c of parent.children) {
        if (toggleTreeNode(c, id)) return true;
      }
    }
    return false;
  }

  // --- Multi-Column Slot Router ---
  function renderSlots() {
    for (let slotIdx = 1; slotIdx <= 3; slotIdx++) {
      const slotEl = document.getElementById(`slot-${slotIdx}`);
      const contentEl = document.getElementById(`slot-content-${slotIdx}`);
      if (!slotEl || !contentEl) continue;

      const bufName = state.activeSlots[slotIdx - 1] || 'chat';
      slotEl.setAttribute('data-active-buf', bufName);

      // Create container for buffer if not already present
      contentEl.innerHTML = `<div id="${bufName}-pane-container" style="height:100%; width:100%;"></div>`;

      // Render the specific buffer
      if (bufName === 'chat') renderChatPane();
      else if (bufName === 'code') renderCodePane();
      else if (bufName === 'graph') renderGraphPane();
      else if (bufName === 'tree') renderTreePane();

      // Update slot dropdown
      const select = slotEl.querySelector('.slot-buf-select');
      if (select) select.value = bufName;
    }
  }

  // --- Feedback Inspector (Dev Tool) ---
  function setupInspector() {
    const toggleBtn = document.getElementById('btn-inspect-toggle');
    const highlight = document.getElementById('inspector-highlight');
    const badge = document.getElementById('inspector-badge');
    const modal = document.getElementById('feedback-modal');
    const modalClose = document.getElementById('btn-modal-close');
    const modalCancel = document.getElementById('btn-modal-cancel');
    const feedbackForm = document.getElementById('feedback-form');
    const targetInput = document.getElementById('feedback-target');

    function toggleInspectMode() {
      state.inspectMode = !state.inspectMode;
      toggleBtn.classList.toggle('active', state.inspectMode);
      if (!state.inspectMode) {
        highlight.classList.add('hidden');
      } else {
        showToast('Inspector Mode Active: Click any element to report feedback');
      }
    }

    toggleBtn.onclick = toggleInspectMode;

    document.addEventListener('mousemove', (e) => {
      if (!state.inspectMode || !modal.classList.contains('hidden')) return;

      const target = document.elementFromPoint(e.clientX, e.clientY);
      if (!target || target.closest('#feedback-modal') || target.closest('#inspector-highlight') || target.closest('#btn-inspect-toggle')) {
        highlight.classList.add('hidden');
        return;
      }

      const inspectId = target.getAttribute('data-inspect-id') || target.closest('[data-inspect-id]')?.getAttribute('data-inspect-id') || target.tagName.toLowerCase();
      state.inspectTarget = {
        element: target,
        id: inspectId,
        tagName: target.tagName.toLowerCase(),
        classes: target.className
      };

      const rect = target.getBoundingClientRect();
      highlight.style.top = `${rect.top}px`;
      highlight.style.left = `${rect.left}px`;
      highlight.style.width = `${rect.width}px`;
      highlight.style.height = `${rect.height}px`;
      highlight.classList.remove('hidden');

      badge.textContent = inspectId;
    });

    document.addEventListener('click', (e) => {
      if (!state.inspectMode) return;
      if (e.target.closest('#btn-inspect-toggle') || e.target.closest('#feedback-modal')) return;

      e.preventDefault();
      e.stopPropagation();

      if (state.inspectTarget) {
        targetInput.value = state.inspectTarget.id;
        modal.classList.remove('hidden');
        document.getElementById('feedback-comment').focus();
      }
    }, true);

    function closeModal() {
      modal.classList.add('hidden');
      highlight.classList.add('hidden');
    }

    modalClose.onclick = closeModal;
    modalCancel.onclick = closeModal;

    feedbackForm.onsubmit = async (e) => {
      e.preventDefault();
      const target = targetInput.value;
      const category = document.getElementById('feedback-category').value;
      const comment = document.getElementById('feedback-comment').value;

      try {
        const res = await fetch('/api/dev/feedback', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            target: { id: target },
            category,
            comment,
            context: {
              activeLayout: state.activeLayout,
              activeSlots: state.activeSlots
            }
          })
        });

        if (res.ok) {
          const result = await res.json();
          showToast(`Feedback saved (#${result.total_feedback_count}) to dev_feedback.json`);
          document.getElementById('feedback-comment').value = '';
          closeModal();
          toggleInspectMode();
        } else {
          showToast('Failed to save feedback: ' + res.statusText);
        }
      } catch (err) {
        showToast('Error saving feedback: ' + err.message);
      }
    };
  }

  // --- Keyboard Shortcuts & Layout Preset Switcher ---
  function setupNavigationControls() {
    // Top Buffer Switchers (Keys: C, V, G, T)
    document.querySelectorAll('.btn-buf').forEach(btn => {
      btn.onclick = () => {
        const buf = btn.getAttribute('data-buf');
        document.querySelectorAll('.btn-buf').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        state.activeSlots[0] = buf;
        renderSlots();
      };
    });

    // Mobile Bottom Nav
    document.querySelectorAll('.mobile-nav-btn').forEach(btn => {
      btn.onclick = () => {
        const buf = btn.getAttribute('data-buf');
        document.querySelectorAll('.mobile-nav-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        state.activeSlots[0] = buf;
        renderSlots();
      };
    });

    // Desktop Layout Presets (1-Col, 2-Col, 3-Col)
    document.querySelectorAll('.btn-preset').forEach(btn => {
      btn.onclick = () => {
        const cols = parseInt(btn.getAttribute('data-cols'), 10);
        document.querySelectorAll('.btn-preset').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');

        state.activeLayout = cols;
        const ws = document.getElementById('workspace');
        ws.className = `workspace layout-${cols}-col`;

        // Adjust visible slots
        document.getElementById('slot-2').style.display = cols >= 2 ? 'flex' : 'none';
        document.getElementById('slot-3').style.display = cols >= 3 ? 'flex' : 'none';
      };
    });

    // Per-Slot Buffer Selectors
    document.querySelectorAll('.slot-buf-select').forEach((select, idx) => {
      select.onchange = (e) => {
        state.activeSlots[idx] = e.target.value;
        renderSlots();
      };
    });

    // Directed Global Keybindings
    document.addEventListener('keydown', (e) => {
      // Don't trigger if user is typing inside input or textarea
      if (['INPUT', 'TEXTAREA', 'SELECT'].includes(e.target.tagName)) {
        if (e.key === 'Escape') {
          e.target.blur();
        }
        return;
      }

      // Ctrl+Shift+C: Toggle Inspector
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'C' || e.key === 'c')) {
        e.preventDefault();
        document.getElementById('btn-inspect-toggle').click();
        return;
      }

      // Direct Single-Key Swaps
      if (e.key === 'c' || e.key === 'C') {
        state.activeSlots[0] = 'chat';
        renderSlots();
      } else if (e.key === 'v' || e.key === 'V') {
        state.activeSlots[0] = 'code';
        renderSlots();
      } else if (e.key === 'g' || e.key === 'G') {
        state.activeSlots[0] = 'graph';
        renderSlots();
      } else if (e.key === 't' || e.key === 'T') {
        state.activeSlots[0] = 'tree';
        renderSlots();
      } else if (e.key === '1') {
        document.querySelector('.btn-preset[data-cols="1"]')?.click();
      } else if (e.key === '2') {
        document.querySelector('.btn-preset[data-cols="2"]')?.click();
      } else if (e.key === '3') {
        document.querySelector('.btn-preset[data-cols="3"]')?.click();
      }
    });
  }

  // --- Toast Notification Helper ---
  function showToast(msg) {
    const toast = document.getElementById('toast');
    if (!toast) return;
    toast.textContent = msg;
    toast.classList.remove('hidden');
    setTimeout(() => {
      toast.classList.add('hidden');
    }, 3200);
  }

  function escapeHtml(str) {
    if (!str) return '';
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }

  // --- Initial Boot Sequence ---
  window.addEventListener('DOMContentLoaded', () => {
    initWebSocket();
    loadModelsFromDriver();
    setupInspector();
    setupNavigationControls();
    loadCodeFile('Cargo.toml');
    renderSlots();
  });
})();
