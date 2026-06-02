from __future__ import annotations
import json
from typing import Dict, Any, List, Optional, Union, Tuple
from .models import UsageNode, ComponentRegistryMap, Component, ComponentTypesMap
from .validator import ArchitectureValidator, resolve_implements_signature

LinesList = List[str]
ErrorList = List[str]
StringDict = Dict[str, str]


class Visualizer:
    """Provides high-fidelity, dual-agent-human visualization of component architecture trees.

    Supports structured ASCII trees for CLI outputs and color-coded interactive Mermaid graphs.
    """

    @staticmethod
    def find_node(node: UsageNode, target_id: str) -> Optional[UsageNode]:
        """Recursively finds a specific UsageNode in a workflow tree by its unique ID."""
        if node.node_id == target_id:
            return node
        for child in node.dependencies:
            res = Visualizer.find_node(child, target_id)
            if res is not None:
                return res
        return None

    @classmethod
    def render_tree_text(
        cls,
        node: UsageNode,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap,
        prefix: str = "",
        is_last: bool = True,
        verbose: str = "summary",
        level: int = 0
    ) -> LinesList:
        """Recursively renders a usage tree in premium ASCII structure.

        Args:
            node: Root UsageNode to render.
            components: Map of registered component definitions.
            component_types: Map of dynamic component type capabilities.
            prefix: Formatting prefix for indentation lines.
            is_last: True if node is the last child of its parent.
            verbose: Verbosity level: 'summary', 'detailed', 'full'.
            level: Recursive depth tracker.
        """
        lines: LinesList = []
        connector: str = "└── " if is_last else "├── "
        if level == 0:
            connector = ""

        # 1. Run local validation for the node
        node_errors: ErrorList = []
        comp: Optional[Component] = components.get(node.component_id)
        if not comp:
            node_errors.append(f"Component '{node.component_id}' not found")
        elif node.expected_inputs is not None or node.expected_outputs is not None or node.expected_side_effects:
            actual_inputs, actual_outputs = resolve_implements_signature(comp, components)
            compat = ArchitectureValidator.verify_compatibility(
                expected_inputs=node.expected_inputs,
                expected_outputs=node.expected_outputs,
                expected_side_effects=node.expected_side_effects,
                actual_inputs=actual_inputs,
                actual_outputs=actual_outputs,
                actual_side_effects=comp.side_effects,
                components=components,
                component_types=component_types,
            )
            if not compat.is_compatible:
                node_errors.extend(compat.errors)

        status_str: str = "🟢 OK" if not node_errors else f"🔴 FAIL ({'; '.join(node_errors)})"
        node_line: str = f"{prefix}{connector}{node.node_id}: {node.caller_id} ──► {node.component_id} [{status_str}]"
        lines.append(node_line)

        # Build sub-indentation prefixes
        child_prefix: str = prefix + ("    " if is_last else "│   ")
        if level == 0:
            child_prefix = ""

        # Detailed metadata presentation
        if verbose in ("detailed", "full"):
            desc: str = node.description or "No description provided."
            lines.append(f"{child_prefix}   Description: {desc}")
            if comp:
                lines.append(f"{child_prefix}   Component: {comp.name} ({comp.type.upper()}) | Status: {comp.status}")
            if node.expected_side_effects:
                se_list: List[str] = [se.target for se in node.expected_side_effects]
                lines.append(f"{child_prefix}   Expected Side-Effects: {se_list or 'None'}")

        # Complete schema specification serialization
        if verbose == "full":
            if node.expected_inputs is not None:
                lines.append(f"{child_prefix}   Expected Inputs Schema:")
                for l in json.dumps(node.expected_inputs, indent=2).splitlines():
                    lines.append(f"{child_prefix}     {l}")
            if node.expected_outputs is not None:
                lines.append(f"{child_prefix}   Expected Outputs Schema:")
                for l in json.dumps(node.expected_outputs, indent=2).splitlines():
                    lines.append(f"{child_prefix}     {l}")
            lines.append(f"{child_prefix}")  # Spacer line

        # 2. Handle child dependency nodes
        num_children: int = len(node.dependencies)
        for i, child in enumerate(node.dependencies):
            lines.extend(
                cls.render_tree_text(
                    child,
                    components,
                    component_types,
                    prefix=prefix + ("    " if is_last else "│   ") if level > 0 else "",
                    is_last=(i == num_children - 1),
                    verbose=verbose,
                    level=level + 1
                )
            )
        return lines

    @classmethod
    def render_tree_mermaid(
        cls,
        node: UsageNode,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap,
        verbose: str = "summary"
    ) -> LinesList:
        """Renders the usage tree as a styled and interactive Mermaid.js flowchart.

        Uses soft premium pastel palettes to match Google Antigravity UI guidelines.
        """
        lines: LinesList = ["graph TD"]
        node_styling: StringDict = {}
        connections: LinesList = []
        node_labels: StringDict = {}

        def traverse(curr: UsageNode) -> None:
            comp: Optional[Component] = components.get(curr.component_id)
            node_errors: ErrorList = []
            if not comp:
                node_errors.append(f"Missing component: {curr.component_id}")
            elif curr.expected_inputs is not None or curr.expected_outputs is not None or curr.expected_side_effects:
                actual_inputs, actual_outputs = resolve_implements_signature(comp, components)
                compat = ArchitectureValidator.verify_compatibility(
                    expected_inputs=curr.expected_inputs,
                    expected_outputs=curr.expected_outputs,
                    expected_side_effects=curr.expected_side_effects,
                    actual_inputs=actual_inputs,
                    actual_outputs=actual_outputs,
                    actual_side_effects=comp.side_effects,
                    components=components,
                    component_types=component_types,
                )
                if not compat.is_compatible:
                    node_errors.extend(compat.errors)

            is_ok: bool = len(node_errors) == 0
            node_styling[curr.node_id] = "ok" if is_ok else "fail"

            # Construct HTML label block
            label_parts: LinesList = [f"<b>{curr.node_id}</b>", f"<code>{curr.caller_id}</code> ──► <code>{curr.component_id}</code>"]
            if verbose in ("detailed", "full"):
                if curr.description:
                    label_parts.append(f"<i>{curr.description}</i>")
                if not is_ok:
                    escaped_errors: List[str] = [err.replace('"', "'") for err in node_errors]
                    label_parts.append(f"<font color='red'>ERROR: {'; '.join(escaped_errors)}</font>")

            label_text: str = "<br/>".join(label_parts)
            node_labels[curr.node_id] = f'{curr.node_id}["{label_text}"]'

            for child in curr.dependencies:
                connections.append(f"  {curr.node_id} --> {child.node_id}")
                traverse(child)

        traverse(node)

        # Output labels
        for nid, label in node_labels.items():
            lines.append(f"  {label}")

        lines.append("")
        # Output connections
        lines.extend(connections)

        lines.append("")
        # Style classes (Premium Pastel Palette)
        lines.append("  classDef ok fill:#e2f0d9,stroke:#2b5717,stroke-width:2px,color:#2b5717;")
        lines.append("  classDef fail fill:#fce4d6,stroke:#c00000,stroke-width:2px,color:#c00000;")

        for nid, cls in node_styling.items():
            lines.append(f"  class {nid} {cls};")

        return lines

    @classmethod
    def render_architecture_mermaid(
        cls,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap
    ) -> LinesList:
        """Generates a premium, grouped Mermaid diagram of the structural component architecture.

        This uses dynamic subgraph nesting for parent namespaces and dashed arrows for interface realizations.
        """
        lines: LinesList = ["graph TB"]
        
        # Index children by their parent_id
        parent_to_children: Dict[Optional[str], List[Component]] = {}
        for comp in components.values():
            parent_to_children.setdefault(comp.parent_id, []).append(comp)
            
        # Recursive subgraph printer
        def render_subgraph(parent_id: Optional[str], depth_prefix: str) -> None:
            children = parent_to_children.get(parent_id, [])
            for child in children:
                # If this child is also a parent, render it as a subgraph!
                if child.id in parent_to_children:
                    lines.append(f"{depth_prefix}subgraph {child.id} [\"{child.name} ({child.type.upper()})\"]")
                    render_subgraph(child.id, depth_prefix + "  ")
                    lines.append(f"{depth_prefix}end")
                else:
                    lines.append(f"{depth_prefix}{child.id}[\"{child.name}<br/><i>type: {child.type}</i>\"]")
                    
        # Render root level nodes first
        render_subgraph(None, "  ")
        
        lines.append("")
        # Render implements realization dashed connections: concrete -.-> abstract
        for comp in components.values():
            if comp.implements_id:
                lines.append(f"  {comp.id} -.->|implements| {comp.implements_id}")
                
        # Premium styling
        lines.append("")
        lines.append("  classDef module fill:#e8f0fe,stroke:#1a73e8,stroke-width:2px,color:#1a73e8;")
        lines.append("  classDef clss fill:#fef7e0,stroke:#f9ab00,stroke-width:2px,color:#f9ab00;")
        lines.append("  classDef fn fill:#e6fcf5,stroke:#00c07f,stroke-width:2px,color:#00c07f;")
        lines.append("  classDef dat fill:#fce8e6,stroke:#ea4335,stroke-width:2px,color:#ea4335;")
        
        for comp in components.values():
            rule = component_types.get(comp.type)
            if not rule:
                continue
            if comp.type == "module" or comp.type == "interface":
                lines.append(f"  class {comp.id} module;")
            elif comp.type == "class":
                lines.append(f"  class {comp.id} clss;")
            elif rule.allows_signature:
                lines.append(f"  class {comp.id} fn;")
            else:
                lines.append(f"  class {comp.id} dat;")
                
        return lines

    @classmethod
    def render_review_markdown(
        cls,
        tree_name: str,
        target_node: UsageNode,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap
    ) -> LinesList:
        """Generates a complete, high-fidelity Markdown document reviewing the given usage tree and all its components."""
        lines: LinesList = []
        import datetime
        timestamp = datetime.datetime.now().isoformat()

        # 1. Header
        lines.append(f"# 📋 Architecture & Workflow Review: {tree_name}")
        lines.append("")
        lines.append("## 1. Executive Summary")
        
        # 2. Run dynamic tree validation
        errors = ArchitectureValidator.validate_usage_node(
            target_node, components, component_types
        )
        if not errors:
            status_str = "🟢 PASSED (No interface, parenting, or capability mismatches detected)"
        else:
            status_str = f"🔴 FAILED ({len(errors)} interface/integration error(s) detected)"
            
        lines.append(f"- **Target Workflow Tree**: `{tree_name}`")
        lines.append(f"- **Validation Status**: `{status_str}`")
        lines.append(f"- **Generated At**: `{timestamp}`")
        lines.append("")

        if errors:
            lines.append("> [!WARNING]")
            lines.append("> **CRITICAL COMPATIBILITY ERRORS DETECTED:**")
            for err in errors:
                lines.append(f"> - Node `{err.node_id}` calling `{err.component_id}`: {err.details}")
            lines.append("")

        lines.append("> [!NOTE]")
        lines.append("> This is a native AG2.0 review card.")
        lines.append("> To comment on any parameter, schema, step, or invariant:")
        lines.append("> 1. Position your cursor on that specific line.")
        lines.append("> 2. Press `c` to open the comment buffer.")
        lines.append("> 3. Type your instructions and press `Esc` to save.")
        lines.append("> 4. Submit your turn once you are finished to send comments back to the agent!")
        lines.append("")
        lines.append("---")
        lines.append("")

        # 3. Workflow Visualization
        lines.append("## 2. Workflow Visualization")
        lines.append("### 2.1 Dependency Flow Diagram")
        lines.append("```mermaid")
        lines.extend(cls.render_tree_mermaid(target_node, components, component_types, verbose="detailed"))
        lines.append("```")
        lines.append("")
        lines.append("### 2.2 ASCII Dependency Tree")
        lines.append("```")
        lines.extend(cls.render_tree_text(target_node, components, component_types, verbose="summary"))
        lines.append("```")
        lines.append("")
        lines.append("---")
        lines.append("")

        # 4. Collect referenced components recursively
        collected_ids: List[str] = []
        def collect(node: UsageNode) -> None:
            if node.component_id and node.component_id not in collected_ids:
                collected_ids.append(node.component_id)
                comp = components.get(node.component_id)
                if comp and comp.implements_id and comp.implements_id not in collected_ids:
                    collected_ids.append(comp.implements_id)
            for child in node.dependencies:
                collect(child)

        collect(target_node)

        # 5. Component Specifications Rendering
        lines.append("## 3. Referenced Component Contracts")
        lines.append("")
        
        for idx, comp_id in enumerate(collected_ids, 1):
            comp = components.get(comp_id)
            if not comp:
                lines.append(f"### 3.{idx} Component: `{comp_id}` (MISSING)")
                lines.append(f"> [!WARNING]")
                lines.append(f"> Component `{comp_id}` is referenced in the tree but is not registered in the registry.")
                lines.append("")
                continue

            lines.append(f"### 3.{idx} Component: `{comp.id}`")
            lines.append(f"* **Name**: {comp.name}")
            lines.append(f"* **Type**: `{comp.type}` | **Status**: `{comp.status}` | **Stage**: `{comp.stage}`")
            lines.append(f"* **Logical Parent**: `{comp.parent_id or 'None (Root Module)'}`")
            lines.append(f"* **Implements Contract**: `{comp.implements_id or 'None (Direct Contract)'}`")
            lines.append("")

            # 📋 Interfaces & Schemas
            lines.append("#### 📋 Interface & Data Schemas")
            
            # Helper to get signature or implements
            actual_inputs, actual_outputs = resolve_implements_signature(comp, components)
            
            if actual_inputs is not None:
                lines.append("* **Inputs DSL/Schema**:")
                lines.append("```json")
                lines.append(json.dumps(actual_inputs, indent=2))
                lines.append("```")
            else:
                lines.append("* **Inputs DSL/Schema**: `None`")

            if actual_outputs is not None:
                lines.append("* **Outputs DSL/Schema**:")
                lines.append("```json")
                lines.append(json.dumps(actual_outputs, indent=2))
                lines.append("```")
            else:
                lines.append("* **Outputs DSL/Schema**: `None`")

            if comp.properties is not None:
                lines.append("* **State/Properties DSL/Schema**:")
                lines.append("```json")
                lines.append(json.dumps(comp.properties, indent=2))
                lines.append("```")
            else:
                lines.append("* **State/Properties DSL/Schema**: `None`")

            if comp.side_effects:
                lines.append("* **Declared Side Effects**:")
                for se in comp.side_effects:
                    lines.append(f"  - `{se.target}`: {se.description}")
            else:
                lines.append("* **Declared Side Effects**: `None (Pure Calculation)`")
            lines.append("")

            # ⚙️ Implementation Specification
            lines.append("#### ⚙️ Implementation Specification (Work Plan)")
            spec = comp.implementation_spec
            if spec:
                if spec.pattern_or_system:
                    lines.append(f"* **Overarching Pattern**: `{spec.pattern_or_system}`")
                else:
                    lines.append("* **Overarching Pattern**: `None declared`")
                    
                if spec.invariants:
                    lines.append("* **System Invariants**:")
                    for inv in spec.invariants:
                        lines.append(f"  - `[{inv.name}]` ({inv.type}): {inv.description}")
                else:
                    lines.append("* **System Invariants**: `None declared`")
                    
                if spec.logic_steps:
                    lines.append("* **Logical Steps**:")
                    # Sort logic steps by sequence
                    sorted_steps = sorted(spec.logic_steps, key=lambda s: s.sequence)
                    for step in sorted_steps:
                        alg_suffix = f" (Algorithm: {step.algorithm})" if step.algorithm else ""
                        comp_suffix = f" [Complexity: {step.complexity}]" if step.complexity else ""
                        lines.append(f"  {step.sequence}. **{step.name}**: {step.description}{alg_suffix}{comp_suffix}")
                else:
                    lines.append("* **Logical Steps**: `None declared`")
                    
                if spec.validation:
                    lines.append("* **Verification & Tool Rules**:")
                    for val in spec.validation:
                        args_suffix = f" with overrides: {val.args}" if val.args else " with default args"
                        lines.append(f"  - Run tool `{val.tool_id}` on targets: `{val.targets}`{args_suffix}")
                else:
                    lines.append("* **Verification & Tool Rules**: `None declared`")
            else:
                lines.append("*No implementation specification designed yet.*")
            lines.append("")

            # 💬 Reviewer Feedback & Comments
            lines.append("#### 💬 Reviewer Feedback & Comments")
            lines.append("> [!IMPORTANT]")
            lines.append(f"> **Use the checklist and comments section below to leave feedback for `{comp.id}`.**")
            lines.append("")
            lines.append("- [ ] Approved")
            lines.append("- [ ] Request Changes")
            lines.append("- [ ] Pending")
            lines.append("- **Comments & Instructions**:")
            lines.append("  - *Leave your comments, corrections, or implementation directions here.*")
            lines.append("")
            lines.append("---")
            lines.append("")

        return lines

