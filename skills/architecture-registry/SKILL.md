---
name: architecture-registry
description: "Captures, manages, and validates abstract software component interfaces (inputs/outputs/side-effects) and their usage trees. Use this to ensure structural design consistency and prevent LLM implementation drift."
---

# Architecture Registry Skill

This skill enables the agent to maintain a strictly-typed, unified registry of software component definitions and their call/dependency trees. It guarantees architectural compliance by checking interface compatibility and side-effect tag compliance across the system.

Under its modern **pure relational schema**, all software elements—from modules and classes to individual functions and data objects—are stored as flat, dynamically-related nodes that establish logical hierarchy and interface inheritance on-the-fly.

---

## 1. Core Principles of Engagement

1. **Design First**: Before implementing any structural element (function, class, module, interface, enum, etc.), register the component node's definition with its status set to `new` using `add_component`.
2. **Trace Usage Explicitly**: As you write code, document the call sites by adding usage nodes to the system's workflow trees. Each node must declare what interface it expects from its targets using `add_usage_node`.
3. **Continuous Auditing**: Regularly run compatibility checks to programmatically verify that all call-site expectations align perfectly with the defined component schemas and side-effect contracts using `check_compatibility`.

> [!WARNING]
> **ALWAYS USE THE DEDICATED MCP TOOLS FOR REGISTRY EDITS**
> Modifying registry JSON files (e.g., `system_architecture.json`, `test_registry.json`) directly via file-writing tools is strictly avoided. You must always interact with the registry using the first-class MCP tools (`init_registry`, `load_registry`, `add_component`, `update_component`, `add_usage_node`, etc.) to preserve JSON structure integrity and guarantee schema validation.

---

## 2. Affirmative Relational Node Design

To keep the system highly decoupled and robust, follow these structural modeling guidelines:

### A. Model Hierarchies Dynamically (Parent-Pointer Relationships)
Every component exists as a flat node in the registry. Establish parent-child hierarchies on-the-fly by setting the `parent_id` parameter:
* **Classes & Methods**: Register the class component first with `type: "class"`. Then, register each of its methods as a separate, flat component of `type: "function"` and set its `parent_id` to the class's ID.
* **Modules & Classes**: Register the module component first with `type: "module"`. Then, register the class component with its `parent_id` set to the module's ID.

### B. Leverage Signature Inheritance (DRY Contracts)
Avoid duplicating interface schemas for concrete implementations. When a component realizes an abstract interface or operation:
1. Register the abstract contract component (e.g., `type: "operation"` or `type: "interface"`) and specify its expected `inputs_dsl` and `outputs_dsl`.
2. Register the concrete component (e.g., `type: "function"`) and set its `implements_id` to the abstract component's ID.
3. Leave the concrete component's `inputs_dsl` and `outputs_dsl` empty (or `None`). The engine will dynamically inherit and validate the signature from the implemented abstract component.

### C. Utilize Custom Object Types (Dynamic Composition)
When defining properties or signatures that accept complex data objects or custom enums, reference them by their ID:
* Define the data structure or enum as its own flat component (e.g., `type: "data_object"` or `type: "enum"`) with its fields specified in `properties_dsl`.
* Reference that component's ID directly inside other components' `inputs_dsl` or `outputs_dsl` (e.g., `userData: UserProfile`). The validation engine recursively resolves and inlines these custom object types at check time.

---

## 3. Shorthand Type DSL Specifications

When defining properties, inputs, and outputs in the MCP tools, use our **Flat String Shorthand DSL** instead of writing full JSON Schemas. It requires zero nested quotes or escaping, is highly readable, and compiles dynamically.

### Supported Primitives
* **Strings**: `string`, `str`, `text`
* **Numbers**: `integer`, `int`, `number`, `float`, `double`
* **Booleans**: `boolean`, `bool`

### Suffix Modifiers & Custom Object Types
* **Optionals (`?`)**: Suffixing a type with `?` (e.g., `int?`) compiles it into a nullable schema (`anyOf: [type, null]`).
* **Arrays (`[]`)**: Suffixing a type with `[]` (e.g., `string[]`) compiles it into a list schema (`type: array, items: type`).
* **Combinations**: Suffixing `int[]?` defines a nullable list of integers.
* **Custom Object Types**: Any alphanumeric name that is not a primitive (e.g., `PaperMetadata` or `paper_metadata`) compiles recursively as a custom object reference (`{"type": "object", "title": "Identifier"}`). The Architecture Validator recursively resolves these dependencies before checking compatibility.

### Flat String Format (Recommended Option 1)
Pass a simple, comma-separated key-value string:
```json
"inputs_dsl": "userId: int, roles: string[], email: str?"
```

### Fallback JSON Format
For legacy or complex nested structures, standard JSON string objects are also gracefully parsed (though they require careful escaping):
```json
"inputs_dsl": "{\"userId\": \"int\", \"profile\": {\"displayName\": \"string\"}}"
```

## 4. Design & Refactoring Specification Models (ImplementationSpec)

When adding or updating components (especially those with `status: "new"` or `status: "modifying"`), you can supply an optional `implementation_spec` representing a structured contract of internal step-by-step algorithms and strict system invariants.

An `ImplementationSpec` consists of two main arrays:
1. **`logic_steps`**: Sequential, ordered logic blocks detailing the workflow and exact steps required to implement the component.
2. **`invariants`**: Assertions, constraints, and preconditions/postconditions that must remain structurally sound and be enforced during compilation or runtime execution.

### A. JSON / Dictionary Schema Specification

```json
{
  "logic_steps": [
    {
      "sequence": 1,
      "name": "Verify parameters",
      "description": "Ensure inputs are sanitized and within legal range bounds."
    },
    {
      "sequence": 2,
      "name": "Database lookup",
      "description": "Query the user table filtering by userId using secure parameterized statements."
    }
  ],
  "invariants": [
    {
      "name": "non_negative_id",
      "type": "pre_condition",
      "description": "Input 'userId' must be greater than or equal to 0."
    },
    {
      "name": "db_transaction_safe",
      "type": "system_invariant",
      "description": "No uncommitted nested transactions are left open."
    }
  ]
}
```

### B. Structural Validation Constraints

The validator (`check_compatibility` or automated internal triggers) strictly validates these spec components:
* **Sequential Contiguity**: `logic_steps` must start exactly at `sequence: 1` and be contiguous and monotonically increasing (1, 2, 3... N). Missing steps, duplicate indices, or non-positive indices will fail validation.
* **Invariant Type Consistency**: When a component overrides or implements a parent's invariant (matched by a unique `name` string), it must strictly preserve its `type` (e.g. `pre_condition`, `post_condition`, `system_invariant`). Attempting to change an inherited `pre_condition` into a `post_condition` in a child class or implementing class will trigger validation failures.

### C. Packaged Subagents & Multi-Agent Orchestration (Stage 3)

To ensure non-negotiable architectural compliance and keep your main context light, this plugin bundles two pre-configured, native subagents inside its `agents/` directory:

1. **`ComponentPlanner`**
   * **Role**: Analyzes target components, recursively resolves interface dependencies, and designs precise logic steps and invariants.
   * **Permissions**: Access to MCP registry tools (`enable_mcp_tools: true`); write access to code sandbox files disabled.
   * **Mandatory First Step**: Must invoke `compile_component_contract` at startup to obtain the component’s authoritative design context before proposing specifications.

2. **`ComponentImplementer`**
   * **Role**: Sandboxed coder. Implements code first via Test-Driven Development (TDD) and executes strict, local validation commands.
   * **Permissions**: Access to both MCP registry tools (`enable_mcp_tools: true`) and full sandboxed filesystem/terminal capabilities (`enable_write_tools: true`).
   * **Mandatory First Step**: Must invoke `compile_component_contract` at startup to obtain the component's authorized contract and list of required validation tools.

#### Choreography & Execution Loop:
* **Task Spawning**: The parent agent invokes a subagent by passing its unique `TypeName` (`ComponentPlanner` or `ComponentImplementer`) to the `invoke_subagent` tool along with the target Component ID as the prompt (e.g. `Implement component "user_repository"`).
* **Direct Contract Retrieval**: The subagent boots up and immediately runs the `compile_component_contract` tool to fetch its authoritative, fully resolved specification in Markdown. This completely avoids bloat in the parent's main context.
* **Testing & Validation**: The implementer subagent creates its unit tests first, writes compliant core code, and runs the declarative verification commands (e.g. `mypy`, `ruff`, `unittest`) listed under the contract's validations section. Once passing, it reports its verification logs back to the orchestrator.

---


## 5. First-Class MCP Server Tools

Use these native tools directly to manage, validate, and inspect the registry:

* `init_registry`: Initialize a brand new, empty architecture registry file.
* `load_registry`: Load an existing architecture registry file (fails if the file does not exist).
* `add_component`: Add a component definition (`properties_dsl`, `inputs_dsl`, or `outputs_dsl` in flat string format; `side_effects_csv` as "tag:description").
  * **Seeded Types**: `"module"`, `"class"`, `"interface"`, `"function"`, `"operation"`, `"data_object"`, `"enum"`.
  * **Status**: Must be one of: `"new"`, `"existing"`, `"modifying"`, or `"deprecated"`.
* `update_component`: Modify registered components with flat, optional field parameters.
* `delete_component`: Remove unused components (fails if referenced by active flows or child components).
* `add_usage_node`: Track a call site in a workflow tree (`expected_inputs_dsl` and `expected_outputs_dsl` as flat strings).
* `update_usage_node`: Update specific fields of an existing call-site node in a tree (e.g., changing expected inputs/outputs, description, or target component).
* `delete_usage_node`: Remove an existing call-site node and its nested child calls from the usage tree.
* `get_next_actionable_components`: Returns a JSON list of component IDs ready for the specified action type bottom-up. Accepts `action_type` (either `"plan"` or `"implement"`).
* `check_compatibility`: Scans trees and returns structural validation errors.
* `visualize_architecture`: Renders trees as formatted ASCII hierarchies or color-coded Mermaid flowcharts.
* `compile_component_contract`: Compiles a complete, stateful, and authoritative architectural contract for a component, recursively resolving and inlining schemas and invariants.
* `approve_arch`: Run static compatibility checks on a component interface and promote its stage to `ARCH_APPROVED`.
* `plan_component`: Validate contiguous sequence logic and invariant compliance, running topological checks to ensure all upstream dependencies are approved first, and promote to `PLAN_APPROVED`.
* `implement_component`: Execute physical verification testing, ensuring bottom-up that all direct dependencies are fully implemented, and promote to `IMPLEMENTED`.

---

## 6. Standard Agent Workflow Example

### 1. Design & Declare contracts and custom types
First, register a custom data transfer object (DTO) and an abstract operation interface:
* Call `add_component` for the DTO:
  * `id`: `"user_dto"`
  * `name`: `"User Data Object"`
  * `type`: `"data_object"`
  * `description`: `"Structure representing a user profile"`
  * `properties_dsl`: `"username: str, email: str?"`

* Call `add_component` for the abstract operation:
  * `id`: `"db_fetch_operation"`
  * `name`: `"Database Fetch Operation"`
  * `type`: `"operation"`
  * `description`: `"Abstract database query signature"`
  * `inputs_dsl`: `"userId: int"`
  * `outputs_dsl`: `"profile: user_dto"`
  * `side_effects_csv`: `"db:Reads user table"`

### 2. Implement and Group relational nodes
Now, register a class and a concrete method that implements our interface, establishing hierarchy and signature inheritance:
* Call `add_component` for the class:
  * `id`: `"user_repository"`
  * `name`: `"User Repository"`
  * `type`: `"class"`
  * `description`: `"Handles loading and saving of users"`
  * `properties_dsl`: `"db_conn: str"`

* Call `add_component` for the concrete method (parented under the class, implementing the operation):
  * `id`: `"user_repository_get_by_id"`
  * `name`: `"Get User By ID Method"`
  * `type`: `"function"`
  * `parent_id`: `"user_repository"`
  * `implements_id`: `"db_fetch_operation"`
  * `description`: `"Loads user from the DB. Signature is inherited automatically."`

### 3. Trace and Verify usage
Track a caller invoking this repository method:
* Call `add_usage_node`:
  * `tree_name`: `"user_profile_load_flow"`
  * `node_id`: `"controller_calls_repo"`
  * `caller_id`: `"user_controller"`
  * `component_id`: `"user_repository_get_by_id"`
  * `description`: `"Retrieves user for dashboard display"`
  * `expected_inputs_dsl`: `"userId: int"`
  * `expected_outputs_dsl`: `"profile: user_dto"`
  * `expected_side_effects_csv`: `"db:DB query"`

* Call `check_compatibility` to programmatically verify all interface and side-effect expectations align correctly.
* Call `visualize_architecture` with `format="mermaid_components"` to output the complete structural system map.

## 7. Stateful Development Lifecycle & Human Gates

To prevent design-to-implementation drift and coordinate multiple cooperating developer agents (e.g., `ComponentPlanner` and `ComponentImplementer`), the Architecture Registry implements a stateful, gate-protected lifecycle machine, a bottom-up topological orchestrator, and a cascade invalidation model.

### A. Lifecycle Stages

Every registered component progresses sequentially through four main stages:

| Stage | Key Meaning | Requirements to Transition | Action Tools |
|---|---|---|---|
| **`DECLARED`** (Default) | Interface skeleton is registered but not yet finalized. | None. This is the starting stage of any newly registered or modified component. | `add_component`, `update_component` |
| **`ARCH_APPROVED`** | The public interface (inputs, outputs, properties, and side-effects) is validated and frozen. | Passed static interface compatibility checks + explicit developer/user sign-off. | `approve_arch` |
| **`PLAN_APPROVED`** | The sequential internal logic steps, algorithm design, and invariants are fully specified. | Valid contiguous implementation spec + all upstream dependencies are at least `ARCH_APPROVED` + developer/user sign-off. | `plan_component` |
| **`IMPLEMENTED`** | The physical source code is written, locally tested, and verified clean. | Local testing validations pass + all direct upstream dependencies are fully `IMPLEMENTED` + developer/user sign-off. | `implement_component` |

### B. The Three Isolated Phases of Engagement

To maintain a clean, separated development lifecycle, developers and calling agents must divide features into three strictly separated phases:

1. **Phase 1: Architectural Design**
   * **Goal**: Define, register, and wire all new components as `DECLARED` interface nodes, creating the full static system structure.
   * **Exit Criteria**: Run compatibility checks on all active call sites, compile contracts, obtain explicit developer sign-off, and promote all components to `ARCH_APPROVED` stage.

2. **Phase 2: Work Planning**
   * **Goal**: Propose and specify algorithm logic steps and strict invariants for every component bottom-up.
   * **Orchestration**: Call `get_next_actionable_components(action_type="plan")` to find which components are structurally ready to be planned (those with no unapproved transitive dependencies).
   * **Exit Criteria**: Propose the `implementation_spec` for each ready component sequentially, compile contracts, obtain human developer approval, and call `plan_component` to promote them to `PLAN_APPROVED`.

3. **Phase 3: Verification & Implementation**
   * **Goal**: Implement high-quality physical code using Test-Driven Development (TDD) and run validation suites.
   * **Orchestration**: Call `get_next_actionable_components(action_type="implement")` to retrieve the list of components ready for coding bottom-up (those whose direct dependencies are fully implemented).
   * **Exit Criteria**: Write code, run local verification tools, compile the contract, prompt the user for merge approval, and call `implement_component` to transition to `IMPLEMENTED`.

### C. Strict ReAct (Reason-Act) Orchestration Protocol

Calling agents **MUST** execute a clear reasoning step before performing any action or promoting any stage. For each component interaction, explicitly output a brief reasoning thought detailing:
1. **Goal**: The targeted lifecycle phase and component.
2. **Readiness Check**: Verification that the component is returned by `get_next_actionable_components` for the active action type.
3. **Contract Compilation**: Compiling and analyzing the current authoritative contract of the component using `compile_component_contract` before designing or writing any code.
4. **Human Prompts**: Explicitly presenting the compiled contract details to the developer and requesting human sign-off before executing transitions.

### D. Hard Circuit Breaker Constraint

To prevent calling agents from falling into infinite loops or consuming excessive context, we enforce a strict **Circuit Breaker** constraint:
> [!IMPORTANT]
> **CONSECUTIVE FAILURE CIRCUIT BREAKER**
> If any component fails verification (e.g., test suite failure, linter error, or type check issue during implementation) **2 times consecutively**, the agent **MUST** immediately:
> 1. Halt all automated operations on that component.
> 2. Save all current work and preserve all diagnostic and execution logs.
> 3. Stop calling tools and clearly report the issue to the human developer, requesting manual intervention or guidance.

### E. Cascading Invalidation Waves (BFS Propagated Reset)

When an interface signature (inputs, outputs, or implements) is updated (`update_component`), its stage is automatically reset to `DECLARED` to prevent code-to-contract drift. To protect dependent systems, an invalidation wave cascades outward recursively using a Breadth-First Search (BFS) graph traversal:

1. **Immediate Target Reset**: The modified component is reset to `DECLARED`.
2. **Dependent Implementation Downgrade**: Any downstream dependent component currently in the `IMPLEMENTED` stage is downgraded to `PLAN_APPROVED` (demanding re-verification of the physical implementation against the changed interface).
3. **Dependent Plan Downgrade**: Any downstream dependent component currently in the `PLAN_APPROVED` stage is downgraded to `ARCH_APPROVED` (demanding re-verification of the logic specification against the updated dependencies).
4. **Impact Report**: The `update_component` MCP tool returns a detailed list of all downstream components affected/downgraded by the invalidation wave, allowing calling agents to notify developers or schedule automatic re-validation workflows.

---

## 8. Native AG2.0 Review & Artifact Generation (Zero-Token Feedback Loop)

To review complex, multi-tiered component integrations without loading thousands of markdown lines (token bloat) into the chat session, the registry supports direct-to-disk generation of interactive review artifacts and their corresponding metadata sidecars.

### A. Core Workflow & Zero-Token Objective
When a user or agent requests a visual architectural contract review of a workflow tree:
1. **Never dump the raw markdown report into the tool output**.
2. **Instead, export it directly to the local AG2.0 artifact directory**.
3. AG2.0's runtime host will automatically detect the file and sidecar, activate the native artifact visualizer tab, and prompt the human reviewer to leave precise line-annotations using the built-in comments interface.
4. When the human reviewer completes their review, the AG2.0 host system serializes these annotations and provides them to the agent as a clean, structured turn message.

### B. Tooling Parameters & Execution

#### 1. CLI Usage (`cli.py visualize`)
To generate a review artifact from the CLI:
```bash
uv run cli.py visualize --tree <tree_name> [--node-id <node_id>] --format review_markdown --to-artifact-dir <path_to_artifact_dir>
```

#### 2. MCP Server Usage (`visualize_architecture`)
When calling this tool via MCP, supply the `to_artifact_dir` and set `format` to `"review_markdown"`.
* **Example Arguments**:
  ```json
  {
    "tree_name": "auth_flow",
    "format": "review_markdown",
    "to_artifact_dir": "/Users/izo/.gemini/antigravity-cli/brain/6b65bfbc-68b6-4a9c-aecf-0a5973fd408f"
  }
  ```
* **Expected Tool Response**:
  The tool returns a minimal status string with the file URI:
  `"SUCCESS: Active review artifact generated at: file:///absolute/path/to/artifact/auth_flow_review.md. Open the artifact tab to review and annotate."`

### C. Sidecar Metadata Specifications
Writing a review artifact requires writing a companion sidecar metadata file with the exact filename `{file_name}.metadata.json` containing:
```json
{
  "artifactType": "ARTIFACT_TYPE_OTHER",
  "summary": "Architectural contract review of the '{tree_name}' workflow and all referenced component definitions.",
  "updatedAt": "2026-06-02T10:00:00Z",
  "requestFeedback": true
}
```
Setting `"requestFeedback": true` tells the AG2.0 host UI to prompt the user to comment and annotate lines, which are then passed back to the agent in subsequent turns to resolve feedback.

