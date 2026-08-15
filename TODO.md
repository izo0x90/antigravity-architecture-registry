
- Explicit review subagents implementers spawn, separate ones per category (NEED THIS ASAP bro)
    - How faithfully is the design implemented (Critical one, even sub agents invent on non common use cases)
    - Anti patterns, also possibly using AST grep for global patterns?
    - Logic correctness
    - Custom linting one based on AST grep or similar tooling with custom rules built for design

- Issue with dicts not being hashable
```
Bug Report: TypeError: unhashable type: 'dict' in Architecture Registry
- Symptom: Calling get_next_actionable_components or visualize_architecture(format="review_markdown") fails with TypeError: unhashable type: 'dict'.
- Root Cause: In system_architecture.json, component definitions store modification_tasks as a list of dictionaries (e.g., [{"task": "...", "completed": false}]). The tool's topological sorting and graph-traversal logic attempts to insert these component dictionaries directly into a Python set or use them as dict keys. Since Python dict and list types are mutable, they are unhashable, triggering the runtime exception.
- Impacted Components: Any component containing structured modification_tasks or complex properties.
- Immediate Workaround: Convert all modification_tasks in the registry file to a flat list of primitive strings (which are immutable and hashable).
```

```
When you call add_usage_node with parent_node_id, the tool's implementation physicalizes this relationship by pushing the entire child node dictionary inside the parent's "dependencies" array in the JSON file. It does not use flat ID-based references for usage relationships.
However, other query commands in the same tool (like get_next_actionable_components) are buggy: they are not written to correctly traverse or flatten this "dependencies" list. When they traverse it, they try to add those nested dictionary nodes directly into a Python set, triggering the unhashable type: 'dict' crash.
Summary of the Situation
1. No Manual Editing: Nobody edited the registry incorrectly; the nested JSON structure was generated entirely by the add_usage_node tool.
2. The Tool's Conflict: The tool's write command (add_usage_node) structures relationships via nested array dictionaries, but the tool's read commands (get_next_actionable_components and review_markdown visualizer) crash when trying to traverse those nested dictionary nodes.
The Clear Path Forward
By registering each of our call sites as separate parallel roots (without passing a parent_node_id), the tool writes them as flat, independent, depth-0 entries. This completely avoids the buggy recursive parsing branch, unbreaks the tool immediately, and keeps our architecture 100% clean and documented. 
I am in Plan Mode (Read-Only). Please let me know if you would like to proceed with flattening these trees!
```

- Missing location field on mcp
```

The MCP tools  add_component  and  update_component  do not expose a  location  parameter in their schemas, making 
it impossible to write or modify the  location  field in the registry using the first-class MCP tools.
```

- Further biasing needed to make sure orch. agent never approves arch without explicit user request to do so
agent must pause once arch defined and validates
```
I was trying to move quickly to progress the workflow, but in doing so, I bypassed the strict instructions to present 
the contract details to you and obtain your explicit sign- off first.

```

- Need to have a way to prevent the planning and impl. agents from needed to go over existing
components that are not going to be changing, skip existing components with no change to implemented


- Right now model can wonder off the workflow defined in the skill, use skill to only bootstrap 
make tools provide continues guidance


- The requirement to capture further detail as specs seems unclear to model sometimes, stronger prompting
and some sort of programmatic gates needed


- On occasion model doesn't by itself get that it needs to capture the existing code "touch points" where new
functionality will integrate into existing

- When oh when do we trigger auto-compaction does opencode expose this or do we need to wait to v2

- Arch Planning stage
    - Implement logic blocks as independent testible scripts
    - Have a path for visual design, do wireframes in .html artifacts for approval

- Arch review stage
    - Review planned arch. in visual navigator that is driven by chat ui

- Implementation stage
    - Build skeleton of components and allow for a user check gate on just those changes and APIs before 
      fully implementing functionality 
