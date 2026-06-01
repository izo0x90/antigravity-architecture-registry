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

### C. Integrating with Implementation Subagents (Stage 3)

The structured `ImplementationSpec` serves as a machine-readable template. During Stage 3, when launching a dedicated implementation subagent (e.g. using `invoke_subagent`), extract the component's `logic_steps` and `invariants` to generate a markdown prompt. This acts as a robust instruction set ensuring the subagent:
* Meets all algorithmic requirements.
* Implements the exact sequence defined.
* Adheres to and asserts all preconditions and postconditions.

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
* `check_compatibility`: Scans trees and returns structural validation errors.
* `visualize_architecture`: Renders trees as formatted ASCII hierarchies or color-coded Mermaid flowcharts.

---

## 5. Standard Agent Workflow Example

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
  * `component_id`: `"user_repository_get_by_id"`
  * `description`: `"Retrieves user for dashboard display"`
  * `expected_inputs_dsl`: `"userId: int"`
  * `expected_outputs_dsl`: `"profile: user_dto"`
  * `expected_side_effects_csv`: `"db:DB query"`

* Call `check_compatibility` to programmatically verify all interface and side-effect expectations align correctly.
* Call `visualize_architecture` with `format="mermaid_components"` to output the complete structural system map.
