
- Explicit review subagents implementers spawn, separate ones per category (NEED THIS ASAP bro)
    - How faithfully is the design implemented (Critical one, even sub agents invent on non common use cases)
    - Anti patterns, also possibly using AST grep for global patterns?
    - Logic correctness
    - Custom linting one based on AST grep or similar tooling with custom rules built for design

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
