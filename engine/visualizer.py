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
