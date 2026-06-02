# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "pydantic>=2.0.0",
#   "jsonschema>=4.0.0",
# ]
# ///

import argparse
import sys
import json
from pathlib import Path
from typing import List, Union

from engine.engine import RegistryEngine
from engine.models import Component, UsageNode, SideEffect
from engine.dsl_compiler import DSLCompiler
from engine.validator import ArchitectureValidator
from engine.visualizer import Visualizer

CLIArgumentList = List[str]
OptionalCLIArgumentList = Union[CLIArgumentList, None]


def create_parser() -> argparse.ArgumentParser:
    """Creates the command line interface parser configuration."""
    parser = argparse.ArgumentParser(
        description="Architecture Registry CLI for managing and validating software components."
    )
    
    # Global option for specifying target file
    parser.add_argument(
        "--file", "-f",
        default="system_architecture.json",
        help="Path to the system architecture JSON file (default: system_architecture.json)"
    )

    subparsers = parser.add_subparsers(dest="command", required=True, help="Subcommands")

    # 0. init command
    subparsers.add_parser("init", help="Initialize a new empty registry file.")

    # 1. status / check-registry command
    subparsers.add_parser("status", help="Show active registry information.")

    # 2. add-component command
    add_comp = subparsers.add_parser("add-component", help="Register a software component.")
    add_comp.add_argument("--id", required=True, help="Unique identifier (e.g. 'sqlite_user_db_read')")
    add_comp.add_argument("--name", required=True, help="Short human-readable name")
    add_comp.add_argument("--type", help="Component type corresponding to registry rules (e.g., 'class', 'function')")
    add_comp.add_argument("--parent-id", help="Optional logical parent component ID establishing hierarchy")
    add_comp.add_argument("--implements-id", help="Optional abstract component ID this component implements")
    add_comp.add_argument("--description", required=True, help="Detailed description")
    add_comp.add_argument("--status", default="new", choices=["new", "existing", "modifying", "deprecated"], help="Implementation status")
    add_comp.add_argument("--properties-dsl", help="Properties/state schema in JSON shorthand format")
    add_comp.add_argument("--inputs-dsl", help="Inputs schema in JSON shorthand format")
    add_comp.add_argument("--outputs-dsl", help="Outputs schema in JSON shorthand format")
    add_comp.add_argument("--side-effects", help="Comma-separated side effect targets (e.g. 'db:write, network:fetch')")
    add_comp.add_argument("--spec-json", help="Implementation specification (Logic steps, invariants) as a raw JSON string")
    add_comp.add_argument("--spec-file", help="Path to a JSON file containing the implementation specification")
    add_comp.add_argument("--category", help="Backward-compatibility category parameter")

    # 3. delete-component command
    del_comp = subparsers.add_parser("delete-component", help="Remove a component from the registry.")
    del_comp.add_argument("--id", required=True, help="Unique component identifier")

    # 4. add-node command
    add_node = subparsers.add_parser("add-node", help="Insert a call site node to a usage tree.")
    add_node.add_argument("--tree", required=True, help="Designated workflow tree name (e.g. 'auth_flow')")
    add_node.add_argument("--node-id", required=True, help="Unique ID for this call site")
    add_node.add_argument("--caller-id", required=True, help="ID of the registered component initiating the call")
    add_node.add_argument("--component-id", required=True, help="Target component being called")
    add_node.add_argument("--description", required=True, help="Context explaining this usage")
    add_node.add_argument("--parent-id", help="Parent calling node ID if nested")
    add_node.add_argument("--expected-inputs-dsl", help="Shorthand expected inputs schema")
    add_node.add_argument("--expected-outputs-dsl", help="Shorthand expected outputs schema")
    add_node.add_argument("--expected-side-effects", help="Comma-separated expected side effects")

    # 4a. update-node command
    up_node = subparsers.add_parser("update-node", help="Update specific fields of an existing usage node.")
    up_node.add_argument("--tree", required=True, help="Designated workflow tree name")
    up_node.add_argument("--node-id", required=True, help="ID of the node to update")
    up_node.add_argument("--caller-id", help="New calling component ID")
    up_node.add_argument("--component-id", help="New target component ID")
    up_node.add_argument("--description", help="New description")
    up_node.add_argument("--expected-inputs-dsl", help="New shorthand expected inputs schema")
    up_node.add_argument("--expected-outputs-dsl", help="New shorthand expected outputs schema")
    up_node.add_argument("--expected-side-effects", help="New comma-separated expected side effects")

    # 4b. delete-node command
    del_node = subparsers.add_parser("delete-node", help="Delete a usage node and all its nested children.")
    del_node.add_argument("--tree", required=True, help="Designated workflow tree name")
    del_node.add_argument("--node-id", required=True, help="ID of the node to delete")

    # 5. validate command
    subparsers.add_parser("validate", help="Run compatibility checks on all workflow usage trees.")

    # 5a. approve-arch command
    approve_arch = subparsers.add_parser("approve-arch", help="Approve the architecture interface of a component.")
    approve_arch.add_argument("id", help="Unique component identifier")
    approve_arch.add_argument("--yes", "-y", action="store_true", help="Auto-approve without human-in-the-loop confirmation prompt")

    # 5b. plan-component command
    plan_comp = subparsers.add_parser("plan-component", help="Approve the work plan/implementation spec of a component.")
    plan_comp.add_argument("id", help="Unique component identifier")
    plan_comp.add_argument("--yes", "-y", action="store_true", help="Auto-approve without human-in-the-loop confirmation prompt")

    # 5c. implement-component command
    implement_comp = subparsers.add_parser("implement-component", help="Validate and approve merging implementation for a component.")
    implement_comp.add_argument("id", help="Unique component identifier")
    implement_comp.add_argument("--yes", "-y", action="store_true", help="Auto-approve without human-in-the-loop confirmation prompt")

    # 6. visualize command
    vis = subparsers.add_parser("visualize", help="Visualize a registered workflow usage tree or components layout.")
    vis.add_argument("--tree", help="The name of the workflow usage tree to visualize. If omitted, all trees or the structural view is visualized.")
    vis.add_argument("--node-id", help="The specific node ID to start the visualization from (visualizes a subtree).")
    vis.add_argument("--format", default="text", choices=["text", "mermaid", "mermaid_components", "review_markdown"], help="Output visualization format (default: text).")
    vis.add_argument("--verbose", default="summary", choices=["summary", "detailed", "full"], help="Level of metadata verbosity (default: summary).")
    vis.add_argument("--to-artifact-dir", help="Optional path to write review markdown and metadata sidecars directly to the artifact directory.")

    # 7. register-tool command
    reg_tool = subparsers.add_parser("register-tool", help="Register a global validation tool definition.")
    reg_tool.add_argument("--id", required=True, help="Unique tool identifier (e.g. 'ruff')")
    reg_tool.add_argument("--executable", required=True, help="Path/binary name (e.g. 'ruff', 'mypy')")
    reg_tool.add_argument("--default-args", nargs="*", default=[], help="Default process arguments (e.g. 'check', '{targets}')")

    # 8. list-tools command
    subparsers.add_parser("list-tools", help="List all registered global validation tools.")

    # 9. update-component command
    update_comp = subparsers.add_parser("update-component", help="Update an existing component's metadata.")
    update_comp.add_argument("--id", required=True, help="Unique component identifier to update")
    update_comp.add_argument("--name", help="Updated human-readable name")
    update_comp.add_argument("--type", help="Updated component type corresponding to registry rules (e.g., 'class', 'function')")
    update_comp.add_argument("--parent-id", "--parent", help="Updated logical parent component ID (or empty string to remove)")
    update_comp.add_argument("--implements-id", "--implements", help="Updated abstract component ID this component implements (or empty string to remove)")
    update_comp.add_argument("--description", help="Updated description")
    update_comp.add_argument("--status", choices=["new", "existing", "modifying", "deprecated"], help="Updated implementation status")
    update_comp.add_argument("--properties-dsl", "--properties", help="Updated properties/state schema in JSON shorthand format")
    update_comp.add_argument("--inputs-dsl", "--inputs", help="Updated inputs schema in JSON shorthand format")
    update_comp.add_argument("--outputs-dsl", "--outputs", help="Updated outputs schema in JSON shorthand format")
    update_comp.add_argument("--side-effects", help="Comma-separated side effect targets (or empty string to remove)")
    update_comp.add_argument("--spec-json", help="Updated implementation specification as a raw JSON string")
    update_comp.add_argument("--spec-file", help="Path to a JSON file containing the updated implementation specification")

    return parser


def parse_side_effects(se_csv: str | None) -> list:
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


def main(argv: OptionalCLIArgumentList = None) -> int:
    parser = create_parser()
    args = parser.parse_args(argv)

    # Initialize Engine
    file_path = Path(args.file).resolve()
    engine = RegistryEngine(file_path)
    
    # Do not attempt to load if initializing a new file
    if args.command == "init":
        try:
            engine.init()
            print(f"Successfully initialized empty registry at '{engine.file_path}'")
            return 0
        except Exception as exc:
            print(f"Error initializing registry: {exc}", file=sys.stderr)
            return 1
    
    try:
        engine.load()
    except Exception as exc:
        print(f"Error loading registry file: {exc}", file=sys.stderr)
        return 1

    if args.command == "status":
        print(f"Registry File: {engine.file_path}")
        print(f"Total Components: {len(engine.registry.components)}")
        for comp_id, comp in engine.registry.components.items():
            print(f"  - [{comp.type.upper()}] {comp_id}: {comp.name}")
            if comp.implementation_spec and comp.implementation_spec.validation:
                print("    Active Validation Tools:")
                for ref in comp.implementation_spec.validation:
                    args_str = f", args={ref.args}" if ref.args is not None else ""
                    print(f"      * {ref.tool_id}: targets={ref.targets}{args_str}")
        print(f"Total Usage Trees: {len(engine.registry.usage_trees)}")
        for tree_name in engine.registry.usage_trees:
            print(f"  - {tree_name}")
        return 0

    elif args.command == "add-component":
        # Resolve component type with backward compatibility
        comp_type = args.type
        if not comp_type and args.category:
            if args.category == "callable":
                comp_type = "function"
            elif args.category == "container":
                comp_type = "module"
            else:
                comp_type = args.category
        
        if not comp_type:
            print("Error: Either --type or --category (deprecated) must be supplied.", file=sys.stderr)
            return 1

        properties_schema = None
        if args.properties_dsl:
            try:
                # If input looks like JSON, load it, otherwise treat as simple shorthand string
                try:
                    parsed = json.loads(args.properties_dsl)
                except ValueError:
                    # Parse simple "key:val" format
                    parsed = {}
                    for item in args.properties_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                properties_schema = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing properties DSL: {exc}", file=sys.stderr)
                return 1

        inputs_schema = None
        if args.inputs_dsl:
            try:
                try:
                    parsed = json.loads(args.inputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.inputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                inputs_schema = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing inputs DSL: {exc}", file=sys.stderr)
                return 1

        outputs_schema = None
        if args.outputs_dsl:
            try:
                try:
                    parsed = json.loads(args.outputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.outputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                outputs_schema = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing outputs DSL: {exc}", file=sys.stderr)
                return 1

        side_effects = parse_side_effects(args.side_effects)

        spec = None
        if args.spec_json:
            try:
                spec_dict = json.loads(args.spec_json)
                from engine.models import ImplementationSpec
                spec = ImplementationSpec.model_validate(spec_dict)
            except Exception as exc:
                print(f"Error parsing --spec-json: {exc}", file=sys.stderr)
                return 1
        elif args.spec_file:
            try:
                spec_path = Path(args.spec_file).resolve()
                with open(spec_path, "r", encoding="utf-8") as f:
                    spec_dict = json.load(f)
                from engine.models import ImplementationSpec
                spec = ImplementationSpec.model_validate(spec_dict)
            except Exception as exc:
                print(f"Error reading/parsing --spec-file: {exc}", file=sys.stderr)
                return 1

        comp = Component(
            id=args.id,
            name=args.name,
            type=comp_type,
            parent_id=args.parent_id,
            implements_id=args.implements_id,
            description=args.description,
            status=args.status,
            properties=properties_schema,
            inputs=inputs_schema,
            outputs=outputs_schema,
            side_effects=side_effects,
            implementation_spec=spec
        )

        try:
            engine.add_component(comp)
            engine.save()
            print(f"Component '{args.id}' added and saved successfully.")
        except Exception as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        return 0

    elif args.command == "delete-component":
        try:
            engine.delete_component(args.id)
            engine.save()
            print(f"Component '{args.id}' deleted successfully.")
        except Exception as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        return 0

    elif args.command == "add-node":
        expected_inputs = None
        if args.expected_inputs_dsl:
            try:
                try:
                    parsed = json.loads(args.expected_inputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.expected_inputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                expected_inputs = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing expected inputs DSL: {exc}", file=sys.stderr)
                return 1

        expected_outputs = None
        if args.expected_outputs_dsl:
            try:
                try:
                    parsed = json.loads(args.expected_outputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.expected_outputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                expected_outputs = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing expected outputs DSL: {exc}", file=sys.stderr)
                return 1

        expected_se = parse_side_effects(args.expected_side_effects)

        node = UsageNode(
            node_id=args.node_id,
            caller_id=args.caller_id,
            component_id=args.component_id,
            description=args.description,
            expected_inputs=expected_inputs,
            expected_outputs=expected_outputs,
            expected_side_effects=expected_se,
            dependencies=[]
        )

        try:
            engine.add_usage_node(args.tree, args.parent_id, node)
            engine.save()
            print(f"Usage node '{args.node_id}' added successfully to tree '{args.tree}'.")
        except Exception as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        return 0

    elif args.command == "update-node":
        updates = {}
        if args.caller_id is not None:
            updates["caller_id"] = args.caller_id
        if args.component_id is not None:
            updates["component_id"] = args.component_id
        if args.description is not None:
            updates["description"] = args.description

        if args.expected_inputs_dsl is not None:
            try:
                try:
                    parsed = json.loads(args.expected_inputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.expected_inputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                updates["expected_inputs"] = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing expected inputs DSL: {exc}", file=sys.stderr)
                return 1

        if args.expected_outputs_dsl is not None:
            try:
                try:
                    parsed = json.loads(args.expected_outputs_dsl)
                except ValueError:
                    parsed = {}
                    for item in args.expected_outputs_dsl.split(","):
                        if ":" in item:
                            k, v = item.split(":", 1)
                            parsed[k.strip()] = v.strip()
                updates["expected_outputs"] = DSLCompiler.compile_shorthand(parsed)
            except Exception as exc:
                print(f"Error parsing expected outputs DSL: {exc}", file=sys.stderr)
                return 1

        if args.expected_side_effects is not None:
            updates["expected_side_effects"] = parse_side_effects(args.expected_side_effects)

        try:
            engine.update_usage_node(args.tree, args.node_id, updates)
            engine.save()
            print(f"Usage node '{args.node_id}' updated successfully.")
        except Exception as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        return 0

    elif args.command == "delete-node":
        try:
            engine.delete_usage_node(args.tree, args.node_id)
            engine.save()
            print(f"Usage node '{args.node_id}' deleted successfully.")
        except Exception as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
        return 0

    elif args.command == "validate":
        all_errors = []
        
        # 1. Run global registry-wide invariant checks first
        global_errors = ArchitectureValidator.validate_registry(engine.registry)
        for err in global_errors:
            all_errors.append(f"Global Registry: {err.details} (Component: '{err.component_id}')")
            
        # 2. Run usage trees verification
        for tree_name, root_node in engine.registry.usage_trees.items():
            errors = ArchitectureValidator.validate_usage_node(
                root_node, engine.registry.components, engine.registry.component_types
            )
            for err in errors:
                all_errors.append(f"Tree '{tree_name}': {err.details} (Node: '{err.node_id}', Component: '{err.component_id}')")

        if not all_errors:
            print("SUCCESS: Compatibility verified! All components and flow trees are valid.")
            return 0
        else:
            print("COMPATIBILITY ERROR(S) DETECTED:", file=sys.stderr)
            for err in all_errors:
                print(f"- {err}", file=sys.stderr)
            return 1

    elif args.command == "visualize":
        if args.format == "mermaid_components" or (not args.tree and not args.node_id and not engine.registry.usage_trees and args.format == "mermaid"):
            print("\n--- Component Architecture Registry ---")
            rendered_lines = Visualizer.render_architecture_mermaid(
                engine.registry.components, engine.registry.component_types
            )
            print("\n".join(rendered_lines))
            return 0

        # Find which trees to render
        trees_to_render = {}
        if args.tree:
            root_node = engine.registry.usage_trees.get(args.tree)
            if not root_node:
                print(f"Error: Tree '{args.tree}' does not exist in registry.", file=sys.stderr)
                return 1
            trees_to_render[args.tree] = root_node
        else:
            trees_to_render = engine.registry.usage_trees

        if not trees_to_render:
            print("No workflow usage trees registered.", file=sys.stderr)
            return 0

        if args.to_artifact_dir:
            if args.format != "review_markdown":
                print("Error: --to-artifact-dir can only be used with --format review_markdown.", file=sys.stderr)
                return 1
            
            import datetime
            artifact_path = Path(args.to_artifact_dir)
            if not artifact_path.exists():
                artifact_path.mkdir(parents=True, exist_ok=True)
                
            for tree_name, root_node in trees_to_render.items():
                target_node = root_node
                if args.node_id:
                    found = Visualizer.find_node(root_node, args.node_id)
                    if not found:
                        continue
                    target_node = found
                
                rendered_lines = Visualizer.render_review_markdown(
                    tree_name, target_node, engine.registry.components, engine.registry.component_types
                )
                
                md_file_path = artifact_path / f"{tree_name}_review.md"
                meta_file_path = artifact_path / f"{tree_name}_review.md.metadata.json"
                
                # Write Markdown
                md_file_path.write_text("\n".join(rendered_lines), encoding="utf-8")
                
                # Write Sidecar Metadata
                metadata = {
                    "artifactType": "ARTIFACT_TYPE_OTHER",
                    "summary": f"Architectural contract review of the '{tree_name}' workflow and all referenced component definitions (schemas, side-effects, status, and implementation specifications).",
                    "updatedAt": datetime.datetime.now().isoformat() + "Z",
                    "requestFeedback": True
                }
                meta_file_path.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
                print(f"SUCCESS: Created review artifact {tree_name}_review.md inside {args.to_artifact_dir}.")
                
            return 0

        for tree_name, root_node in trees_to_render.items():
            target_node = root_node
            if args.node_id:
                found = Visualizer.find_node(root_node, args.node_id)
                if not found:
                    if args.tree:
                        print(f"Error: Node '{args.node_id}' not found in tree '{args.tree}'.", file=sys.stderr)
                        return 1
                    continue
                target_node = found

            print(f"\n--- Workflow Tree: {tree_name} ---")
            if args.format == "mermaid":
                rendered_lines = Visualizer.render_tree_mermaid(
                    target_node, engine.registry.components, engine.registry.component_types, verbose=args.verbose
                )
                print("\n".join(rendered_lines))
            elif args.format == "review_markdown":
                rendered_lines = Visualizer.render_review_markdown(
                    tree_name, target_node, engine.registry.components, engine.registry.component_types
                )
                print("\n".join(rendered_lines))
            else:
                rendered_lines = Visualizer.render_tree_text(
                    target_node, engine.registry.components, engine.registry.component_types, verbose=args.verbose
                )
                print("\n".join(rendered_lines))

        return 0

    elif args.command == "approve-arch":
        comp_id = args.id
        comp = engine.registry.components.get(comp_id)
        if not comp:
            print(f"Error: Component '{comp_id}' does not exist in registry.", file=sys.stderr)
            return 1
            
        # 1. Programmatic Gate
        from engine.validator import check_component_compatibility, compile_contract_markdown
        errors = check_component_compatibility(engine.registry, comp_id)
        if errors:
            print(f"Programmatic architecture check failed for '{comp_id}':", file=sys.stderr)
            for err in errors:
                print(f"- {err}", file=sys.stderr)
            return 1
            
        # 2. Human Gate
        print("\n--- Compiled Architectural Contract ---")
        contract = compile_contract_markdown(comp, engine.registry)
        print(contract)
        print("---------------------------------------\n")
        
        if not args.yes:
            resp = input(f"Do you approve the architecture interface for '{comp_id}'? (y/n): ")
            if resp.strip().lower() not in ("y", "yes"):
                print("Architecture approval cancelled by user.")
                return 1
                
        # 3. Result
        from engine.models import LifecycleStage
        comp.stage = LifecycleStage.ARCH_APPROVED
        engine.save()
        print(f"Success: Component '{comp_id}' architecture interface approved! Stage set to ARCH_APPROVED.")
        return 0

    elif args.command == "plan-component":
        comp_id = args.id
        comp = engine.registry.components.get(comp_id)
        if not comp:
            print(f"Error: Component '{comp_id}' does not exist in registry.", file=sys.stderr)
            return 1
            
        # 1. Prerequisite Check
        from engine.models import LifecycleStage
        if comp.stage != LifecycleStage.ARCH_APPROVED:
            print(f"Error: Component '{comp_id}' must be in ARCH_APPROVED stage before planning. Current stage: {comp.stage.upper()}.", file=sys.stderr)
            return 1
            
        # 2. Topological Guard
        from engine.validator import get_component_dependencies
        visited = set()
        queue = get_component_dependencies(comp)
        declared_deps = []
        while queue:
            curr = queue.pop(0)
            if curr in visited:
                continue
            visited.add(curr)
            dep_comp = engine.registry.components.get(curr)
            if dep_comp:
                if dep_comp.stage == LifecycleStage.DECLARED:
                    declared_deps.append(curr)
                for dep in get_component_dependencies(dep_comp):
                    if dep not in visited:
                        queue.append(dep)
                        
        if declared_deps:
            print(f"Error: Topological guard block. The following dependencies of '{comp_id}' are still in DECLARED stage. Please run approve-arch on them first:", file=sys.stderr)
            for dep in declared_deps:
                print(f"- {dep}", file=sys.stderr)
            return 1
            
        # 3. Programmatic Gate (contiguous steps, overridden invariants)
        from engine.validator import check_component_compatibility, compile_contract_markdown
        errors = check_component_compatibility(engine.registry, comp_id)
        if errors:
            print(f"Programmatic work plan check failed for '{comp_id}':", file=sys.stderr)
            for err in errors:
                print(f"- {err}", file=sys.stderr)
            return 1
            
        # 4. Human Gate
        print("\n--- Compiled Work Plan Contract ---")
        contract = compile_contract_markdown(comp, engine.registry)
        print(contract)
        print("------------------------------------\n")
        
        if not args.yes:
            resp = input(f"Do you approve the work plan for '{comp_id}'? (y/n): ")
            if resp.strip().lower() not in ("y", "yes"):
                print("Work plan approval cancelled by user.")
                return 1
                
        # 5. Result
        comp.stage = LifecycleStage.PLAN_APPROVED
        engine.save()
        print(f"Success: Work plan approved for '{comp_id}'! Stage set to PLAN_APPROVED.")
        return 0

    elif args.command == "implement-component":
        comp_id = args.id
        comp = engine.registry.components.get(comp_id)
        if not comp:
            print(f"Error: Component '{comp_id}' does not exist in registry.", file=sys.stderr)
            return 1
            
        # 1. Prerequisite Check
        from engine.models import LifecycleStage
        if comp.stage != LifecycleStage.PLAN_APPROVED:
            print(f"Error: Component '{comp_id}' must be in PLAN_APPROVED stage before implementing. Current stage: {comp.stage.upper()}.", file=sys.stderr)
            return 1
            
        # 2. Topological Guard
        from engine.validator import get_component_dependencies
        direct_deps = get_component_dependencies(comp)
        non_implemented_deps = []
        for dep in direct_deps:
            dep_comp = engine.registry.components.get(dep)
            if dep_comp and dep_comp.stage != LifecycleStage.IMPLEMENTED:
                non_implemented_deps.append((dep, dep_comp.stage))
                
        if non_implemented_deps:
            print(f"Error: Topological guard block. Direct dependencies of '{comp_id}' must be in IMPLEMENTED stage before implementation can proceed. Missing direct dependencies:", file=sys.stderr)
            for dep, stage in non_implemented_deps:
                print(f"- {dep} (Current Stage: {stage.upper()})", file=sys.stderr)
            return 1
            
        # 3. Programmatic Gate
        from engine.validator import run_component_validations
        success, logs = run_component_validations(comp, engine.registry)
        print("\n--- Validation Logs ---")
        print(logs)
        print("------------------------\n")
        
        if not success:
            print(f"Error: Programmatic verification failed for component '{comp_id}'. Validation tools returned non-zero exit codes.", file=sys.stderr)
            return 1
            
        # 4. Human Gate
        if not args.yes:
            resp = input(f"Do you approve merging the implementation for '{comp_id}'? (y/n): ")
            if resp.strip().lower() not in ("y", "yes"):
                print("Implementation merge cancelled by user.")
                return 1
                
        # 5. Result
        comp.stage = LifecycleStage.IMPLEMENTED
        engine.save()
        print(f"Success: Component '{comp_id}' implementation verified and merged! Stage set to IMPLEMENTED.")
        return 0

    elif args.command == "register-tool":
        try:
            from engine.models import ValidationToolDefinition
            tool = ValidationToolDefinition(
                id=args.id,
                executable=args.executable,
                default_args=args.default_args
            )
            engine.registry.validation_tools[args.id] = tool
            engine.save()
            print(f"Validation tool '{args.id}' registered successfully.")
            return 0
        except Exception as exc:
            print(f"Error registering validation tool: {exc}", file=sys.stderr)
            return 1

    elif args.command == "list-tools":
        if not engine.registry.validation_tools:
            print("No validation tools registered.")
            return 0
        print("\n--- Registered Validation Tools ---")
        for tool_id, tool in engine.registry.validation_tools.items():
            print(f"- {tool_id}: executable='{tool.executable}', default_args={tool.default_args}")
        return 0

    elif args.command == "update-component":
        updates = {}
        if args.name is not None:
            updates["name"] = args.name
        if args.type is not None:
            updates["type"] = args.type
        if args.parent_id is not None:
            updates["parent_id"] = None if args.parent_id == "" else args.parent_id
        if args.implements_id is not None:
            updates["implements_id"] = None if args.implements_id == "" else args.implements_id
        if args.description is not None:
            updates["description"] = args.description
        if args.status is not None:
            updates["status"] = args.status
        if args.side_effects is not None:
            updates["side_effects"] = [] if args.side_effects == "" else parse_side_effects(args.side_effects)

        if args.properties_dsl is not None:
            if args.properties_dsl == "":
                updates["properties"] = None
            else:
                try:
                    try:
                        parsed = json.loads(args.properties_dsl)
                    except ValueError:
                        parsed = {}
                        for item in args.properties_dsl.split(","):
                            if ":" in item:
                                k, v = item.split(":", 1)
                                parsed[k.strip()] = v.strip()
                    updates["properties"] = DSLCompiler.compile_shorthand(parsed)
                except Exception as exc:
                    print(f"Error parsing properties DSL: {exc}", file=sys.stderr)
                    return 1

        if args.inputs_dsl is not None:
            if args.inputs_dsl == "":
                updates["inputs"] = None
            else:
                try:
                    try:
                        parsed = json.loads(args.inputs_dsl)
                    except ValueError:
                        parsed = {}
                        for item in args.inputs_dsl.split(","):
                            if ":" in item:
                                k, v = item.split(":", 1)
                                parsed[k.strip()] = v.strip()
                    updates["inputs"] = DSLCompiler.compile_shorthand(parsed)
                except Exception as exc:
                    print(f"Error parsing inputs DSL: {exc}", file=sys.stderr)
                    return 1

        if args.outputs_dsl is not None:
            if args.outputs_dsl == "":
                updates["outputs"] = None
            else:
                try:
                    try:
                        parsed = json.loads(args.outputs_dsl)
                    except ValueError:
                        parsed = {}
                        for item in args.outputs_dsl.split(","):
                            if ":" in item:
                                k, v = item.split(":", 1)
                                parsed[k.strip()] = v.strip()
                    updates["outputs"] = DSLCompiler.compile_shorthand(parsed)
                except Exception as exc:
                    print(f"Error parsing outputs DSL: {exc}", file=sys.stderr)
                    return 1

        if args.spec_json is not None:
            if args.spec_json == "":
                updates["implementation_spec"] = None
            else:
                try:
                    spec_dict = json.loads(args.spec_json)
                    from engine.models import ImplementationSpec
                    updates["implementation_spec"] = ImplementationSpec.model_validate(spec_dict)
                except Exception as exc:
                    print(f"Error parsing --spec-json: {exc}", file=sys.stderr)
                    return 1
        elif args.spec_file is not None:
            if args.spec_file == "":
                updates["implementation_spec"] = None
            else:
                try:
                    spec_path = Path(args.spec_file).resolve()
                    with open(spec_path, "r", encoding="utf-8") as f:
                        spec_dict = json.load(f)
                    from engine.models import ImplementationSpec
                    updates["implementation_spec"] = ImplementationSpec.model_validate(spec_dict)
                except Exception as exc:
                    print(f"Error reading/parsing --spec-file: {exc}", file=sys.stderr)
                    return 1

        try:
            downgraded = engine.update_component(args.id, updates)
            engine.save()
            print(f"Component '{args.id}' updated and saved successfully.")
            if downgraded:
                print("Cascading invalidation downgraded the following components to DECLARED stage:")
                for cid in downgraded:
                    print(f"  - {cid}")
            return 0
        except Exception as exc:
            print(f"Error updating component '{args.id}': {exc}", file=sys.stderr)
            return 1

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
