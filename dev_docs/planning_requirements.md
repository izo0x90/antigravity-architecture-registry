# Antigravity 2.0 Task-Breakdown & Planning Requirements

This document outlines the strict design guidelines and execution requirements for orchestrating component task-breakdowns and planning within the Antigravity 2.0 multi-agent system.

---

## Core Requirements

### 1. Incremental, Component-by-Component Planning (No "One-Shotting")
* **Rule**: Planning must never be executed as a single "big bang" or one-shot task-breakdown for the entire system or multi-component design.
* **Mechanism**: Task breakdown must be negotiated **one-by-one or in small batches of at most one component at a time**. This guarantees fine-grained accuracy and prevents planning drift or hallucinations.

### 2. MCP-Bound Registry Boundaries
* **Rule**: The Architecture Registry remains the absolute single source of truth.
* **Mechanism**: All reads and writes to components, implementation specs, states, and dependency trees must proceed strictly through the defined FastMCP tools (e.g., `add_component`, `update_component`, `visualize_architecture`, `check_compatibility`). Directly editing registry JSON files via file-writing tools is strictly prohibited.

### 3. Context Negotiation (Zero Main-Context Bloat)
* **Rule**: The Main Orchestrator Agent must maintain a compact, long-lived coordination context.
* **Mechanism**: When delegating task planning to a planner subagent, the Main Orchestrator provides the *narrowest possible slice of context* necessary to plan that specific component (e.g., the component's logical properties, parent structure, abstract interface, and its `ImplementationSpec` consisting of sequential logic steps and invariants). This keeps the main context lightweight, highly focused, and immune to token saturation.

### 4. Guaranteed Follow-Through in Topological Order
* **Rule**: Implementation tasks must be planned and executed in strict bottom-up dependency order.
* **Mechanism**: The Main Orchestrator queries the registry to obtain a **topological sort** of the component graph. It then systematically drives the planning and execution queue starting bottom-up (independent utility/leaf components first, then their direct dependents, up to the top-level controllers or orchestrators). It tracks the status of every single node in the registry, ensuring absolute follow-through with no omitted components.
