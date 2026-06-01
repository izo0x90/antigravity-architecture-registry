# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "pydantic>=2.0.0",
#   "jsonschema>=4.0.0",
#   "mcp>=0.1.0",
# ]
# ///

import json
from pathlib import Path
from typing import Optional, Union, List
from mcp.server.fastmcp import FastMCP, Context

from engine.engine import RegistryEngine
from engine.models import Component, UsageNode, SideEffect
from engine.dsl_compiler import DSLCompiler
from engine.validator import ArchitectureValidator
from engine.visualizer import Visualizer

# Explicit type aliases for the MCP interface layer
ToolResponseText = str
OptionalParentNodeID = Union[str, None]

# 1. Initialize the FastMCP Server (no hardcoded startup file, completely dynamic!)
mcp = FastMCP("Architecture Registry")


async def get_isolated_engine(ctx: Context) -> RegistryEngine:
    """Helper to fetch or lazy-initialize the RegistryEngine instance bound to this active session.

    This prevents cross-client conversation bleeding and keeps session storage encapsulated.
    """
    engine = getattr(ctx.session, "active_engine_instance", None)
    if engine is None:
        # Graceful default: load system_architecture.json in current working directory
        default_path = Path.cwd() / "system_architecture.json"
        engine = RegistryEngine(default_path)
        if not default_path.exists():
            engine.init()
        else:
            engine.load()
        ctx.session.active_engine_instance = engine
    return engine


def parse_side_effects(se_csv: Optional[str]) -> List[SideEffect]:
    """Helper to parse custom side-effects comma-separated format: 'db:writes to DB, network:requests'."""
    if not se_csv:
        return []
    results = []
    for token in se_csv.split(","):
        if ":" in token:
            tag, desc = token.split(":", 1)
            results.append(SideEffect(target=tag.strip(), description=desc.strip()))
        else:
            tag_clean = token.strip()
            results.append(SideEffect(target=tag_clean, description=f"Side effect on {tag_clean}"))
    return results


def parse_implementation_spec(spec_arg: Optional[Union[str, dict]]) -> Optional[dict]:
    """Helper to parse raw string or dictionary representing ImplementationSpec."""
    if not spec_arg:
        return None
    if isinstance(spec_arg, dict):
        return spec_arg
    if isinstance(spec_arg, str):
        spec_arg = spec_arg.strip()
        if not spec_arg:
            return None
        try:
            return json.loads(spec_arg)
        except Exception as exc:
            raise ValueError(f"Failed to parse implementation spec JSON string: {str(exc)}")
    return None


def parse_shorthand_str(shorthand_str: Optional[str]) -> Optional[Union[str, dict]]:
    """Robustly parses a shorthand DSL string into a dictionary or primitive type string.

    Supports:
      - Clean comma-separated key-value: "userId: int, email: str?"
      - Standard JSON string (fallback): '{"userId": "int", "email": "str"}'
      - Primitive types: "int", "str[]?", etc.
    """
    if not shorthand_str:
        return None
    shorthand_str = shorthand_str.strip()
    if not shorthand_str:
        return None

    # Fallback to standard JSON parsing if it looks like a JSON object or array
    if (shorthand_str.startswith("{") and shorthand_str.endswith("}")) or (
        shorthand_str.startswith("[") and shorthand_str.endswith("]")
    ):
        try:
            return json.loads(shorthand_str)
        except Exception:
            pass

    # If it is a primitive type (no colons at all), return as-is
    if ":" not in shorthand_str:
        return shorthand_str

    # Parse as comma-separated key-value pairs
    parsed = {}
    for part in shorthand_str.split(","):
        part = part.strip()
        if not part:
            continue
        if ":" in part:
            k, v = part.split(":", 1)
            parsed[k.strip()] = v.strip()
        else:
            # If no colon, treat the token as a string field name by default
            parsed[part.strip()] = "str"
    return parsed


# ==========================================
# MCP Exposed Tools (Session Isolated)
# ==========================================


@mcp.tool()
async def load_registry(file_path: str, ctx: Context) -> ToolResponseText:
    """Loads a specific architecture registry file (fails if the file does not exist).

    Args:
        file_path: Relative or absolute path to the target JSON registry file.
    """
    target_path = Path(file_path).resolve()
    engine = RegistryEngine(target_path)
    try:
        engine.load()
    except Exception as exc:
        return f"Error loading registry: {str(exc)}"
    
    # Securely store in session context
    ctx.session.active_engine_instance = engine
    return f"Successfully loaded registry file: {target_path}"


@mcp.tool()
async def init_registry(file_path: str, ctx: Context) -> ToolResponseText:
    """Initializes a brand new, empty architecture registry file.

    Args:
        file_path: Relative or absolute path to the new JSON registry file.
    """
    target_path = Path(file_path).resolve()
    engine = RegistryEngine(target_path)
    try:
        engine.init()
    except Exception as exc:
        return f"Error initializing registry: {str(exc)}"
    
    # Securely store in session context
    ctx.session.active_engine_instance = engine
    return f"Successfully initialized new registry file: {target_path}"


@mcp.tool()
async def check_active_registry(ctx: Context) -> ToolResponseText:
    """Returns details of the currently loaded active architecture registry in this session."""
    engine = await get_isolated_engine(ctx)
    num_components = len(engine.registry.components)
    num_trees = len(engine.registry.usage_trees)
    return (
        f"Active registry: {engine.file_path}\n"
        f"Total Registered Components: {num_components}\n"
        f"Total Active Flow Trees: {num_trees}"
    )


@mcp.tool()
async def add_component(
    id: str,
    name: str,
    type: str,
    description: str,
    parent_id: Optional[str] = None,
    implements_id: Optional[str] = None,
    status: str = "new",
    properties_dsl: Optional[str] = None,
    inputs_dsl: Optional[str] = None,
    outputs_dsl: Optional[str] = None,
    side_effects_csv: Optional[str] = None,
    implementation_spec: Optional[Union[str, dict]] = None,
    category: Optional[str] = None,  # Backward-compatibility fallback
    ctx: Context = None,
) -> ToolResponseText:
    """Register a new software component in the active registry.

    Args:
        id: Unique alphanumeric identifier (e.g., 'sqlite_user_db_read').
        name: Short functional name (e.g., 'read').
        type: Corresponds strictly to a dynamic registry component_types key (e.g., 'module', 'class', 'function', 'data_object').
        description: Detailed behavioral description.
        parent_id: Optional logical parent namespace ID, establishing hierarchy.
        implements_id: Optional abstract component ID this component realizes/implements.
        status: Progress state ('new', 'existing', 'modifying', 'deprecated').
        properties_dsl: Optional shorthand DSL string specifying component properties/state.
        inputs_dsl: Optional shorthand DSL string of parameters/arguments.
        outputs_dsl: Optional shorthand DSL string of return values.
        side_effects_csv: Optional comma-separated side-effects (e.g. 'db:write, fs:logs').
        implementation_spec: Optional implementation spec JSON string or object (dict) describing sequential logic and invariants.
            The dictionary format must match:
            {
              "logic_steps": [
                {
                  "sequence": int (Starting from 1, strictly contiguous: 1, 2, 3...),
                  "name": "str (Name/summary of the logic step)",
                  "description": "str (Detailed algorithms or logic to execute)"
                }
              ],
              "invariants": [
                {
                  "name": "str (Unique invariant identifier name)",
                  "type": "str (Must be one of 'pre_condition', 'post_condition', 'system_invariant')",
                  "description": "str (The actual invariant or condition rule to enforce)"
                }
              ]
            }
            CRITICAL RULES:
              1. Logic steps sequence must be exactly unique and contiguous starting from 1 to N (e.g., [1, 2, 3]). Missing, duplicate, or out-of-order sequence indices fail validation.
              2. Overridden invariants (inheriting from parent 'implements_id' interfaces with matching name) must strictly preserve their invariant type (e.g., a pre_condition cannot be overridden as a post_condition).
        category: Backward compatibility parameter, mapped to 'type' if type is omitted.
    """
    if ctx is None:
        return "Error: Context parameter is required."
        
    engine = await get_isolated_engine(ctx)
    
    # Graceful backward-compatibility mapping
    if not type and category:
        if category == "callable":
            type = "function"
        elif category == "container":
            type = "module"
        else:
            type = category

    properties_schema = None
    if properties_dsl:
        try:
            parsed = parse_shorthand_str(properties_dsl)
            properties_schema = DSLCompiler.compile_shorthand(parsed)
        except Exception as exc:
            return f"Error compiling properties DSL: {str(exc)}"

    inputs_schema = None
    if inputs_dsl:
        try:
            parsed_inputs = parse_shorthand_str(inputs_dsl)
            inputs_schema = DSLCompiler.compile_shorthand(parsed_inputs)
        except Exception as exc:
            return f"Error compiling inputs DSL: {str(exc)}"

    outputs_schema = None
    if outputs_dsl:
        try:
            parsed_outputs = parse_shorthand_str(outputs_dsl)
            outputs_schema = DSLCompiler.compile_shorthand(parsed_outputs)
        except Exception as exc:
            return f"Error compiling outputs DSL: {str(exc)}"

    side_effects = parse_side_effects(side_effects_csv)

    spec = None
    if implementation_spec:
        try:
            spec_dict = parse_implementation_spec(implementation_spec)
            if spec_dict:
                from engine.models import ImplementationSpec
                spec = ImplementationSpec.model_validate(spec_dict)
        except Exception as exc:
            return f"Error parsing/validating implementation spec: {str(exc)}"
    
    comp = Component(
        id=id,
        name=name,
        type=type,
        parent_id=parent_id,
        implements_id=implements_id,
        description=description,
        status=status,
        properties=properties_schema,
        inputs=inputs_schema,
        outputs=outputs_schema,
        side_effects=side_effects,
        implementation_spec=spec,
    )
    
    try:
        engine.add_component(comp)
        engine.save()
    except Exception as exc:
        return f"Error registering component: {str(exc)}"
        
    return f"Component '{id}' registered successfully and saved to '{engine.file_path}'."


@mcp.tool()
async def update_component(
    id: str,
    name: Optional[str] = None,
    type: Optional[str] = None,
    parent_id: Optional[str] = None,
    implements_id: Optional[str] = None,
    description: Optional[str] = None,
    status: Optional[str] = None,
    properties_dsl: Optional[str] = None,
    inputs_dsl: Optional[str] = None,
    outputs_dsl: Optional[str] = None,
    side_effects_csv: Optional[str] = None,
    implementation_spec: Optional[Union[str, dict]] = None,
    category: Optional[str] = None,  # Backward-compatibility fallback
    ctx: Context = None,
) -> ToolResponseText:
    """Updates fields of an existing registered component.

    Args:
        id: Unique identifier of the target component.
        name: Optional updated human-readable name.
        type: Optional updated component type key.
        parent_id: Optional updated logical parent namespace ID.
        implements_id: Optional updated abstract component ID implemented.
        description: Optional updated detailed behavioral description.
        status: Optional updated progress state ('new', 'existing', 'modifying', 'deprecated').
        properties_dsl: Optional updated shorthand DSL specifying component properties/state.
        inputs_dsl: Optional updated shorthand DSL string of inputs.
        outputs_dsl: Optional updated shorthand DSL string of outputs.
        side_effects_csv: Optional updated comma-separated side-effects.
        implementation_spec: Optional updated implementation spec JSON string or object (dict) describing sequential logic and invariants.
            The dictionary format must match:
            {
              "logic_steps": [
                {
                  "sequence": int (Starting from 1, strictly contiguous: 1, 2, 3...),
                  "name": "str (Name/summary of the logic step)",
                  "description": "str (Detailed algorithms or logic to execute)"
                }
              ],
              "invariants": [
                {
                  "name": "str (Unique invariant identifier name)",
                  "type": "str (Must be one of 'pre_condition', 'post_condition', 'system_invariant')",
                  "description": "str (The actual invariant or condition rule to enforce)"
                }
              ]
            }
            CRITICAL RULES:
              1. Logic steps sequence must be exactly unique and contiguous starting from 1 to N (e.g., [1, 2, 3]). Missing, duplicate, or out-of-order sequence indices fail validation.
              2. Overridden invariants (inheriting from parent 'implements_id' interfaces with matching name) must strictly preserve their invariant type (e.g., a pre_condition cannot be overridden as a post_condition).
        category: Backward compatibility parameter, mapped to 'type' if type is omitted.
    """
    if ctx is None:
        return "Error: Context parameter is required."
        
    engine = await get_isolated_engine(ctx)
    
    if not type and category:
        if category == "callable":
            type = "function"
        elif category == "container":
            type = "module"
        else:
            type = category

    updates = {}
    if name is not None:
        updates["name"] = name
    if type is not None:
        updates["type"] = type
    if parent_id is not None:
        updates["parent_id"] = parent_id
    if implements_id is not None:
        updates["implements_id"] = implements_id
    if description is not None:
        updates["description"] = description
    if status is not None:
        updates["status"] = status

    if properties_dsl is not None:
        try:
            parsed = parse_shorthand_str(properties_dsl)
            updates["properties"] = DSLCompiler.compile_shorthand(parsed) if parsed else None
        except Exception as exc:
            return f"Error compiling properties DSL: {str(exc)}"

    if inputs_dsl is not None:
        try:
            parsed = parse_shorthand_str(inputs_dsl)
            updates["inputs"] = DSLCompiler.compile_shorthand(parsed) if parsed else None
        except Exception as exc:
            return f"Error compiling inputs DSL: {str(exc)}"

    if outputs_dsl is not None:
        try:
            parsed = parse_shorthand_str(outputs_dsl)
            updates["outputs"] = DSLCompiler.compile_shorthand(parsed) if parsed else None
        except Exception as exc:
            return f"Error compiling outputs DSL: {str(exc)}"

    if side_effects_csv is not None:
        se_list = parse_side_effects(side_effects_csv)
        updates["side_effects"] = se_list

    if implementation_spec is not None:
        try:
            spec_dict = parse_implementation_spec(implementation_spec)
            if spec_dict:
                from engine.models import ImplementationSpec
                spec = ImplementationSpec.model_validate(spec_dict)
                updates["implementation_spec"] = spec.model_dump()
            else:
                updates["implementation_spec"] = None
        except Exception as exc:
            return f"Error parsing/validating implementation spec: {str(exc)}"

    try:
        engine.update_component(id, updates)
        engine.save()
    except Exception as exc:
        return f"Error: {str(exc)}"

    return f"Component '{id}' updated successfully in '{engine.file_path}'."


@mcp.tool()
async def delete_component(id: str, ctx: Context = None) -> ToolResponseText:
    """Removes a component from the registry (fails if referenced by active flows or children).

    Args:
        id: Unique identifier of the target component.
    """
    if ctx is None:
        return "Error: Context parameter is required."
        
    engine = await get_isolated_engine(ctx)
    try:
        engine.delete_component(id)
        engine.save()
    except Exception as exc:
        return f"Error: {str(exc)}"
    return f"Component '{id}' deleted successfully from '{engine.file_path}'."


@mcp.tool()
async def add_usage_node(
    tree_name: str,
    node_id: str,
    caller_id: str,
    component_id: str,
    description: str,
    parent_node_id: OptionalParentNodeID = None,
    expected_inputs_dsl: Optional[str] = None,
    expected_outputs_dsl: Optional[str] = None,
    expected_side_effects_csv: Optional[str] = None,
    ctx: Context = None,
) -> ToolResponseText:
    """Inserts a usage/call node into a designated dependency flow tree.

    Args:
        tree_name: Identifier for the flow (e.g. 'user_sign_up_flow').
        node_id: Unique identifier for this call site (e.g., 'controller_calls_service').
        caller_id: ID of the registered component initiating the call.
        component_id: Target component being called.
        description: Context explaining this specific usage.
        parent_node_id: The ID of the calling node, if this is a nested sub-call.
        expected_inputs_dsl: Optional shorthand DSL string of caller's input expectations.
        expected_outputs_dsl: Optional shorthand DSL string of caller's output expectations.
        expected_side_effects_csv: Optional comma-separated side-effects expected at this call-site.
    """
    if ctx is None:
        return "Error: Context parameter is required."
        
    engine = await get_isolated_engine(ctx)
    
    expected_inputs = None
    if expected_inputs_dsl:
        try:
            parsed_inputs = parse_shorthand_str(expected_inputs_dsl)
            expected_inputs = DSLCompiler.compile_shorthand(parsed_inputs)
        except Exception as exc:
            return f"Error compiling expected inputs DSL: {str(exc)}"
 
    expected_outputs = None
    if expected_outputs_dsl:
        try:
            parsed_outputs = parse_shorthand_str(expected_outputs_dsl)
            expected_outputs = DSLCompiler.compile_shorthand(parsed_outputs)
        except Exception as exc:
            return f"Error compiling expected outputs DSL: {str(exc)}"
 
    expected_se = parse_side_effects(expected_side_effects_csv)
    
    node = UsageNode(
        node_id=node_id,
        caller_id=caller_id,
        component_id=component_id,
        description=description,
        expected_inputs=expected_inputs,
        expected_outputs=expected_outputs,
        expected_side_effects=expected_se,
        dependencies=[]
    )

    try:
        engine.add_usage_node(tree_name, parent_node_id, node)
        engine.save()
    except Exception as exc:
        return f"Error adding usage node: {str(exc)}"

    return f"Usage node '{node_id}' successfully added to tree '{tree_name}'."


@mcp.tool()
async def check_compatibility(ctx: Context = None) -> ToolResponseText:
    """Runs strict semantic interface and global registry-wide invariant checks."""
    if ctx is None:
        return "Error: Context parameter is required."
        
    engine = await get_isolated_engine(ctx)
    all_errors = []

    # 1. Run global registry verification first
    global_errors = ArchitectureValidator.validate_registry(engine.registry)
    for err in global_errors:
        all_errors.append(f"[Global Registry] Component '{err.component_id}': {err.details}")

    # 2. Run usage trees validation
    for tree_name, root_node in engine.registry.usage_trees.items():
        errors = ArchitectureValidator.validate_usage_node(
            root_node, engine.registry.components, engine.registry.component_types
        )
        for err in errors:
            all_errors.append(f"[{tree_name}] Node '{err.node_id}'/Component '{err.component_id}': {err.details}")

    if not all_errors:
        return f"COMPATIBILITY VERIFIED: No interface, parenting, or capability mismatches detected in '{engine.file_path}'."

    return f"COMPATIBILITY FAILURES DETECTED IN '{engine.file_path}':\n" + "\n".join(f"- {e}" for e in all_errors)


@mcp.tool()
async def visualize_architecture(
    tree_name: Optional[str] = None,
    node_id: Optional[str] = None,
    format: str = "text",
    verbose: str = "summary",
    ctx: Context = None
) -> ToolResponseText:
    """Generates ASCII hierarchy trees, rich flowchart diagrams, or structural component registry graphs.

    Args:
        tree_name: Name of a specific usage tree (e.g. 'auth_flow'). If omitted, all trees or the structural view is rendered.
        node_id: Specific starting node ID to visualize a subtree.
        format: Output format: 'text' (ASCII tree), 'mermaid' (Mermaid flowchart of workflows), or 'mermaid_components' (structural component architecture map). Default: 'text'.
        verbose: Information level: 'summary', 'detailed', or 'full'. Default: 'summary'.
        ctx: Context parameter for session tracking.
    """
    if ctx is None:
        return "Error: Context parameter is required."

    engine = await get_isolated_engine(ctx)

    if format == "mermaid_components" or (not tree_name and not node_id and not engine.registry.usage_trees and format == "mermaid"):
        rendered = Visualizer.render_architecture_mermaid(
            engine.registry.components, engine.registry.component_types
        )
        return "### Component Architecture Registry\n```mermaid\n" + "\n".join(rendered) + "\n```"

    trees_to_render = {}
    if tree_name:
        root_node = engine.registry.usage_trees.get(tree_name)
        if not root_node:
            return f"Error: Tree '{tree_name}' does not exist in registry."
        trees_to_render[tree_name] = root_node
    else:
        trees_to_render = engine.registry.usage_trees

    if not trees_to_render:
        return f"No workflow usage trees registered in '{engine.file_path}'."

    output_blocks = []
    for t_name, root_node in trees_to_render.items():
        target_node = root_node
        if node_id:
            found = Visualizer.find_node(root_node, node_id)
            if not found:
                if tree_name:
                    return f"Error: Node '{node_id}' not found in tree '{tree_name}'."
                continue
            target_node = found

        output_blocks.append(f"### Workflow Tree: {t_name}")
        if format == "mermaid":
            rendered = Visualizer.render_tree_mermaid(
                target_node, engine.registry.components, engine.registry.component_types, verbose=verbose
            )
            output_blocks.append("```mermaid\n" + "\n".join(rendered) + "\n```")
        else:
            rendered = Visualizer.render_tree_text(
                target_node, engine.registry.components, engine.registry.component_types, verbose=verbose
            )
            output_blocks.append("```\n" + "\n".join(rendered) + "\n```")

    return "\n\n".join(output_blocks)


if __name__ == "__main__":
    mcp.run()
